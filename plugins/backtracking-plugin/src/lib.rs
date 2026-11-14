#[cfg(feature = "editor")]
mod editor;
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
mod session;
mod settings;

use std::num::NonZeroU32;
use std::sync::Arc;

use nice_plug::audio_setup::ProcessMode;
use nice_plug::prelude::*;

#[cfg(feature = "editor")]
use crate::editor::CaptureEditor;
use crate::session::{Session, Worker};
use crate::settings::SettingsStore;

const MONO_AUX: [NonZeroU32; 1] = [NonZeroU32::new(1).unwrap()];
const STEREO_AUX: [NonZeroU32; 1] = [NonZeroU32::new(2).unwrap()];
const SIDECHAIN_NAMES: PortNames = PortNames {
    aux_inputs: &["Sidechain"],
    ..PortNames::const_default()
};

const fn layout(
    main: u32,
    sidechain: &'static [NonZeroU32],
    names: PortNames,
) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: NonZeroU32::new(main),
        main_output_channels: NonZeroU32::new(main),
        aux_input_ports: sidechain,
        names,
        ..AudioIOLayout::const_default()
    }
}

struct Backtracking {
    params: Arc<PluginParams>,
    session: Arc<Session>,
    worker: Option<Worker>,
    capture_enabled: bool,
}

#[derive(Params)]
struct PluginParams {
    #[persist = "capture_settings_v1"]
    settings: SettingsStore,
}

impl Default for Backtracking {
    fn default() -> Self {
        let settings = SettingsStore::default();
        Self {
            params: Arc::new(PluginParams {
                settings: settings.clone(),
            }),
            session: Arc::new(Session::new(settings)),
            worker: None,
            capture_enabled: false,
        }
    }
}

impl Plugin for Backtracking {
    const NAME: &'static str = "Backtracking";
    const VENDOR: &'static str = "Backtracking";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        layout(2, &STEREO_AUX, SIDECHAIN_NAMES),
        layout(2, &MONO_AUX, SIDECHAIN_NAMES),
        layout(1, &MONO_AUX, SIDECHAIN_NAMES),
        layout(1, &STEREO_AUX, SIDECHAIN_NAMES),
        layout(2, &[], PortNames::const_default()),
        layout(1, &[], PortNames::const_default()),
    ];

    #[cfg(feature = "editor")]
    type Editor = CaptureEditor;
    #[cfg(not(feature = "editor"))]
    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    #[cfg(feature = "editor")]
    fn editor(&mut self, _: AsyncExecutor<Self>) -> Option<Self::Editor> {
        editor::create(self.session.clone())
    }

    fn activate(
        &mut self,
        layout: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        // Nice-plug 0.4.2 snapshots the prior mode in the VST3 setup callback. Treat
        // only CLAP's current mode as authoritative so a bounce cannot disable subsequent capture.
        self.capture_enabled =
            context.plugin_api() != PluginApi::Clap || config.process_mode != ProcessMode::Offline;
        if !self.capture_enabled {
            return true;
        }

        let rate = config.sample_rate.round() as u32;
        let channels = layout.main_input_channels.map_or(0, |n| n.get() as usize);
        let sidechain = layout.aux_input_ports.first().map_or(0, |n| n.get() as usize);
        if !self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.matches(rate, channels, sidechain, config.max_buffer_size))
        {
            // Drop and join the old consumer before changing the format shared with the editor.
            self.worker = None;
            self.worker = Worker::with_sidechain(
                self.session.clone(), rate, channels, sidechain, config.max_buffer_size,
            );
        }
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        auxiliary: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Nice-plug already copies the input to output when the host does not use in-place buffers.
        if self.capture_enabled
            && let Some(worker) = &self.worker
        {
            let sidechain = auxiliary.inputs.first().map(Buffer::as_slice_immutable);
            worker.push_block(
                buffer.as_slice_immutable(),
                sidechain.unwrap_or(&[]),
                bar_frames(context.transport()),
            );
            if context.plugin_api() == PluginApi::Clap && self.session.continuous_processing_requested() {
                return ProcessStatus::KeepAlive;
            }
        }
        ProcessStatus::Normal
    }
}

/// A bar at or after the first sample, and the distance to subsequent bars.
/// The worker turns this compact transport description into packet offsets.
fn bar_frames(transport: &Transport) -> Option<(f64, f64)> {
    if !transport.playing {
        return None;
    }
    let position = transport.pos_beats.filter(|p| p.is_finite())?;
    let start = transport.bar_start_pos_beats.filter(|p| p.is_finite())?;
    let tempo = transport.tempo.filter(|p| p.is_finite() && *p > 0.)?;
    let numerator = transport.time_sig_numerator.filter(|&n| n > 0)?;
    let denominator = transport.time_sig_denominator.filter(|&n| n > 0)?;
    let rate = f64::from(transport.sample_rate);
    let bar_beats = 4. * f64::from(numerator) / f64::from(denominator);
    let samples_per_beat = rate * 60. / tempo;
    let period = bar_beats * samples_per_beat;
    if !period.is_finite() || period < 1. {
        return None;
    }
    let since_bar = (position - start) * samples_per_beat;
    // A host may report the previous bar at its exact right boundary.
    if since_bar < -0.5 || since_bar > period + 0.5 {
        return None;
    }
    let first = if since_bar.abs() <= 0.5 {
        0.
    } else {
        (period - since_bar).max(0.)
    };
    Some((first, period))
}

impl ClapPlugin for Backtracking {
    const CLAP_ID: &'static str = "tw.kamelyang.backtracking";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Retrospective audio capture");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Mono,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Backtracking {
    const VST3_CLASS_ID: [u8; 16] = *b"BktrkSampler0001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

nice_export_vst3!(Backtracking);
nice_export_clap!(Backtracking);

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use backtracking_core::{AutoResumeDelay, DurationChoice, WavExportProfile, WavSampleFormat};
    use nice_plug::context::process::SendEventError;
    use nice_plug::params::persist::serialize_field;

    use super::*;
    use crate::settings::{MeterMode, SessionSettings, WaveformLayout};

    struct AudioContext {
        api: PluginApi,
        transport: Transport,
    }

    impl AudioContext {
        fn new(api: PluginApi) -> Self {
            Self { api, transport: Transport::new(1_000.) }
        }
    }

    impl ActivateContext<Backtracking> for AudioContext {
        fn plugin_api(&self) -> PluginApi { self.api }
        fn execute(&self, _: ()) { panic!("unexpected activation task") }
        fn set_latency_samples(&self, _: u32) { panic!("unexpected latency change") }
        fn set_current_voice_capacity(&self, _: u32) { panic!("unexpected voice capacity change") }
    }

    impl ProcessContext<Backtracking> for AudioContext {
        fn plugin_api(&self) -> PluginApi { self.api }
        fn execute_background(&self, _: ()) { panic!("unexpected background task") }
        fn execute_gui(&self, _: ()) { panic!("unexpected GUI task") }
        fn transport(&self) -> &Transport { &self.transport }
        fn next_event(&mut self) -> Option<PluginNoteEvent<Backtracking>> { None }
        fn try_send_event(&mut self, event: PluginNoteEvent<Backtracking>)
            -> Result<(), (PluginNoteEvent<Backtracking>, SendEventError)>
        {
            Err((event, SendEventError::NoOutputBuffer))
        }
        fn set_latency_samples(&self, _: u32) { panic!("unexpected latency change") }
        fn request_restart(&self) { panic!("unexpected host restart") }
        fn set_current_voice_capacity(&self, _: u32) { panic!("unexpected voice capacity change") }
    }

    fn realtime_config() -> BufferConfig {
        BufferConfig { sample_rate: 1_000., min_buffer_size: Some(4), max_buffer_size: 16,
            process_mode: ProcessMode::Realtime }
    }

    fn quiet_sections(plugin: &Backtracking, enabled: bool) {
        let mut settings = plugin.session.settings();
        settings.retain_silence = enabled;
        plugin.session.update_editor_settings(settings);
    }

    fn sender_test_plugin(api: PluginApi) -> (Backtracking, AudioContext) {
        let mut plugin = Backtracking::default();
        let mut context = AudioContext::new(api);
        assert!(plugin.activate(&layout(2, &MONO_AUX, SIDECHAIN_NAMES), &realtime_config(), &mut context));
        plugin.worker.as_mut().unwrap().stop_background_for_test();
        (plugin, context)
    }

    fn process_preserving_audio(plugin: &mut Backtracking, context: &mut AudioContext, silent: bool) -> ProcessStatus {
        let mut main = if silent {
            vec![vec![0., -0., 0., -0.], vec![-0., 0., -0., 0.]]
        } else {
            vec![vec![0.5, -0.75, 0.00001, -0.], vec![-0.25, 0.125, -0.00001, 0.]]
        };
        let mut side = if silent { vec![0., -0., -0., 0.] } else { vec![0.25, -0.5, 0., -0.] };
        let expected_main = main.clone();
        let expected_side = side.clone();
        let mut buffer = Buffer::default();
        let mut sidechain = Buffer::default();
        // All slices remain live until after process() and share the declared four-frame length.
        unsafe {
            buffer.set_slices(4, |slices| {
                *slices = main.iter_mut().map(Vec::as_mut_slice).collect();
            });
            sidechain.set_slices(4, |slices| *slices = vec![side.as_mut_slice()]);
        }
        let mut inputs = [sidechain];
        let mut outputs = [];
        let mut auxiliary = AuxiliaryBuffers { inputs: &mut inputs, outputs: &mut outputs };
        let status = plugin.process(&mut buffer, &mut auxiliary, context);
        for (actual, expected) in buffer.as_slice_immutable().iter().zip(&expected_main) {
            assert_eq!(actual.iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
                expected.iter().copied().map(f32::to_bits).collect::<Vec<_>>());
        }
        assert_eq!(auxiliary.inputs[0].as_slice_immutable()[0].iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
            expected_side.iter().copied().map(f32::to_bits).collect::<Vec<_>>());
        status
    }

    #[test]
    fn quiet_capture_process_keeps_host_alive_and_preserves_all_audio_buffers() {
        for api in [PluginApi::Clap, PluginApi::Vst3, PluginApi::Standalone] {
            let mut plugin = Backtracking::default();
            let mut context = AudioContext::new(api);
            assert!(plugin.activate(&layout(2, &MONO_AUX, SIDECHAIN_NAMES), &realtime_config(), &mut context));
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
            quiet_sections(&plugin, true);
            let enabled_status = if api == PluginApi::Clap { ProcessStatus::KeepAlive } else { ProcessStatus::Normal };
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), enabled_status);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, false), enabled_status);
            quiet_sections(&plugin, false);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, false), ProcessStatus::Normal);
        }
    }

    #[test]
    fn restored_quiet_capture_keeps_alive_only_with_an_active_capture_worker() {
        let original = Backtracking::default();
        let disabled_fields = original.params().serialize_fields();
        quiet_sections(&original, true);
        let fields = original.params().serialize_fields();
        for api in [PluginApi::Clap, PluginApi::Vst3, PluginApi::Standalone] {
            let mut restored = Backtracking::default();
            restored.params().deserialize_fields(&fields);
            let mut context = AudioContext::new(api);
            assert!(restored.activate(&layout(2, &MONO_AUX, SIDECHAIN_NAMES), &realtime_config(), &mut context));
            let enabled_status = if api == PluginApi::Clap { ProcessStatus::KeepAlive } else { ProcessStatus::Normal };
            assert_eq!(process_preserving_audio(&mut restored, &mut context, true), enabled_status);
            restored.params().deserialize_fields(&disabled_fields);
            assert_eq!(process_preserving_audio(&mut restored, &mut context, true), ProcessStatus::Normal);
            restored.params().deserialize_fields(&fields);
            assert_eq!(process_preserving_audio(&mut restored, &mut context, true), enabled_status);
            if api == PluginApi::Clap {
                let offline = BufferConfig { process_mode: ProcessMode::Offline, ..realtime_config() };
                assert!(restored.activate(&layout(2, &MONO_AUX, SIDECHAIN_NAMES), &offline, &mut context));
                assert_eq!(process_preserving_audio(&mut restored, &mut context, true), ProcessStatus::Normal);
            }
            restored.worker = None;
            restored.capture_enabled = true;
            assert_eq!(process_preserving_audio(&mut restored, &mut context, true), ProcessStatus::Normal);
        }
    }

    #[test]
    fn sender_only_process_keeps_clap_alive_without_enabling_local_quiet_capture() {
        for api in [PluginApi::Clap, PluginApi::Vst3, PluginApi::Standalone] {
            let (mut plugin, mut context) = sender_test_plugin(api);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
            let mut settings = plugin.session.settings();
            settings.sender_enabled = true;
            plugin.session.update_editor_settings(settings);
            assert!(!plugin.session.settings().retain_silence);
            let enabled_status = if api == PluginApi::Clap { ProcessStatus::KeepAlive } else { ProcessStatus::Normal };
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), enabled_status);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, false), enabled_status);

            plugin.session.pause();
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), enabled_status);
            plugin.session.resume();
            quiet_sections(&plugin, true);
            let mut settings = plugin.session.settings();
            settings.sender_enabled = false;
            plugin.session.update_editor_settings(settings);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), enabled_status);
            quiet_sections(&plugin, false);
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
        }
    }

    #[test]
    fn sender_only_host_restore_updates_process_status_before_any_worker_tick() {
        let original = Backtracking::default();
        let disabled = original.params().serialize_fields();
        let mut settings = original.session.settings();
        settings.sender_enabled = true;
        original.session.update_editor_settings(settings);
        let sender_only = original.params().serialize_fields();
        quiet_sections(&original, true);
        let both = original.params().serialize_fields();
        let mut settings = original.session.settings();
        settings.sender_enabled = false;
        original.session.update_editor_settings(settings);
        let quiet_only = original.params().serialize_fields();

        for api in [PluginApi::Clap, PluginApi::Vst3, PluginApi::Standalone] {
            let (mut plugin, mut context) = sender_test_plugin(api);
            let enabled_status = if api == PluginApi::Clap { ProcessStatus::KeepAlive } else { ProcessStatus::Normal };
            for (fields, expected_quiet, status) in [
                (&sender_only, false, enabled_status), (&disabled, false, ProcessStatus::Normal),
                (&both, true, enabled_status), (&quiet_only, true, enabled_status),
                (&disabled, false, ProcessStatus::Normal), (&sender_only, false, enabled_status),
            ] {
                plugin.params().deserialize_fields(fields);
                assert_eq!(plugin.session.settings().retain_silence, expected_quiet);
                assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), status);
                assert_eq!(process_preserving_audio(&mut plugin, &mut context, false), status);
            }
            if api == PluginApi::Clap {
                let offline = BufferConfig { process_mode: ProcessMode::Offline, ..realtime_config() };
                assert!(plugin.activate(&layout(2, &MONO_AUX, SIDECHAIN_NAMES), &offline, &mut context));
                assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
            }
            plugin.worker = None;
            plugin.capture_enabled = true;
            assert_eq!(process_preserving_audio(&mut plugin, &mut context, true), ProcessStatus::Normal);
        }
    }

    #[test]
    fn bars_follow_host_phase_and_are_hidden_without_valid_metadata() {
        let mut transport = Transport::new(48_000.);
        transport.playing = true;
        transport.tempo = Some(120.);
        transport.time_sig_numerator = Some(6);
        transport.time_sig_denominator = Some(8);
        transport.pos_beats = Some(10.75);
        assert_eq!(bar_frames(&transport), None);
        transport.bar_start_pos_beats = Some(9.);
        assert_eq!(bar_frames(&transport), Some((30_000., 72_000.)));
        transport.pos_beats = Some(12.);
        assert_eq!(bar_frames(&transport), Some((0., 72_000.)));
        transport.playing = false;
        assert_eq!(bar_frames(&transport), None);
    }

    #[test]
    fn plugin_state_round_trips_instance_settings_without_audio_or_pause() {
        let original = Backtracking::default();
        original.session.set_retention(DurationChoice::Seconds60);
        original.session.set_idle_resume(AutoResumeDelay::Immediate);
        let mut capture_settings = original.session.settings();
        capture_settings.retain_silence = true;
        original.session.update_editor_settings(capture_settings);
        original.session.set_panel_transparency(75);
        original.session.set_accent([224, 90, 155]);
        original.session.set_meter_mode(MeterMode::Lufs);
        original.session.set_show_waveform_only(true);
        original.session.set_waveform_layout(WaveformLayout::Stereo);
        original.session.set_wav_export(
            WavExportProfile {
                sample_rate: 96_000,
                sample_format: WavSampleFormat::Pcm24,
            },
            true,
        );
        let output = std::env::temp_dir()
            .join("Backtracking 測試")
            .join("output");
        original.session.set_output_dir(output);
        original
            .session
            .set_saved_dir(std::env::temp_dir().join("Backtracking 測試").join("saved"));
        let worker = Worker::new(original.session.clone(), 1_000, 1, 256).unwrap();
        worker.push(&[&mut [0.5; 32]]);
        original.session.pause();
        assert!(!original.session.snapshot().unwrap().samples.is_empty());

        let mut expected = original.session.settings();
        let fields = original.params().serialize_fields();
        let restored = Backtracking::default();
        restored.params().deserialize_fields(&fields);
        let mut restored_settings = restored.session.settings();
        // The live endpoint may finish registering while the DAW reads the settings.
        expected.sender_id = None;
        restored_settings.sender_id = None;
        assert_eq!(restored_settings, expected);

        let _worker = Worker::new(restored.session.clone(), 1_000, 1, 256).unwrap();
        assert!(!restored.session.is_paused());
        assert!(restored.session.snapshot().unwrap().samples.is_empty());
    }

    #[test]
    fn legacy_and_invalid_fields_use_defaults_without_losing_valid_values() {
        let plugin = Backtracking::default();
        let defaults = SessionSettings::default();
        assert!(!defaults.force_mono);
        assert!(!defaults.retain_silence);
        plugin.params().deserialize_fields(&BTreeMap::new());
        assert_eq!(plugin.session.settings(), defaults);

        let mut input = BTreeMap::from([
            ("history_seconds", "30".to_owned()),
            ("retain_silence", "invalid".to_owned()),
            ("idle_resume_seconds", "unsupported".to_owned()),
            ("panel_transparency", "250".to_owned()),
            ("accent_rgb", "[0, 256, 0]".to_owned()),
            ("meter_unit", "VU".to_owned()),
            ("waveform_layout", "unsupported".to_owned()),
            (
                "output_dir",
                serialize_field(std::ffi::OsStr::new("relative/output")).unwrap(),
            ),
        ]);
        plugin.params().deserialize_fields(&BTreeMap::from([(
            "capture_settings_v1".to_owned(),
            serialize_field(&input).unwrap(),
        )]));

        assert_eq!(
            plugin.session.settings(),
            SessionSettings {
                retention: DurationChoice::Seconds30,
                meter_mode: MeterMode::Vu,
                ..defaults.clone()
            }
        );

        input.insert("wav_sample_rate", "192000".to_owned());
        input.insert("wav_bits_per_sample", "16".to_owned());
        plugin.params().deserialize_fields(&BTreeMap::from([(
            "capture_settings_v1".to_owned(),
            serialize_field(&input).unwrap(),
        )]));
        assert_eq!(
            plugin.session.settings().wav_profile,
            WavExportProfile {
                sample_format: WavSampleFormat::Pcm16,
                ..defaults.wav_profile
            }
        );
    }
}
