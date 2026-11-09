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
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
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
        if !self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.matches(rate, channels, config.max_buffer_size))
        {
            // Drop and join the old consumer before changing the format shared with the editor.
            self.worker = None;
            self.worker = Worker::new(self.session.clone(), rate, channels, config.max_buffer_size);
        }
        true
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Nice-plug already copies the input to output when the host does not use in-place buffers.
        if self.capture_enabled
            && let Some(worker) = &self.worker
        {
            worker.push(buffer.as_slice_immutable());
        }
        ProcessStatus::Normal
    }
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
    use nice_plug::params::persist::serialize_field;

    use super::*;
    use crate::settings::{MeterMode, SessionSettings};

    #[test]
    fn plugin_state_round_trips_instance_settings_without_audio_or_pause() {
        let original = Backtracking::default();
        original.session.set_retention(DurationChoice::Seconds30);
        original.session.set_idle_resume(AutoResumeDelay::Immediate);
        original.session.set_panel_transparency(75);
        original.session.set_meter_mode(MeterMode::Lufs);
        original.session.set_show_waveform_only(true);
        original.session.set_wav_profile(WavExportProfile {
            sample_rate: 96_000,
            sample_format: WavSampleFormat::Pcm24,
        });
        let output = std::env::temp_dir()
            .join("Backtracking 測試")
            .join("output");
        original.session.set_output_dir(output);
        let worker = Worker::new(original.session.clone(), 1_000, 1, 256).unwrap();
        worker.push(&[&mut [0.5; 32]]);
        original.session.pause();
        assert!(!original.session.snapshot().unwrap().samples.is_empty());

        let fields = original.params().serialize_fields();
        let restored = Backtracking::default();
        restored.params().deserialize_fields(&fields);
        let _worker = Worker::new(restored.session.clone(), 1_000, 1, 256).unwrap();

        assert_eq!(restored.session.settings(), original.session.settings());
        assert!(!restored.session.is_paused());
        assert!(restored.session.snapshot().unwrap().samples.is_empty());
    }

    #[test]
    fn legacy_and_invalid_fields_use_defaults_without_losing_valid_values() {
        let plugin = Backtracking::default();
        let defaults = SessionSettings::default();
        plugin.params().deserialize_fields(&BTreeMap::new());
        assert_eq!(plugin.session.settings(), defaults);

        let mut input = BTreeMap::from([
            ("history_seconds", "30".to_owned()),
            ("idle_resume_seconds", "unsupported".to_owned()),
            ("panel_transparency", "250".to_owned()),
            ("meter_unit", "VU".to_owned()),
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
