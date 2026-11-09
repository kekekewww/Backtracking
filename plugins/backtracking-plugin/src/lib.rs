#[cfg(feature = "editor")]
mod editor;
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
mod session;

use std::num::NonZeroU32;
use std::sync::Arc;

use nice_plug::audio_setup::ProcessMode;
use nice_plug::prelude::*;

#[cfg(feature = "editor")]
use crate::editor::CaptureEditor;
use crate::session::{Session, Worker};

struct Backtracking {
    params: Arc<PluginParams>,
    session: Arc<Session>,
    worker: Option<Worker>,
    capture_enabled: bool,
}

#[derive(Default, Params)]
struct PluginParams {}

impl Default for Backtracking {
    fn default() -> Self {
        Self {
            params: Arc::new(PluginParams::default()),
            session: Arc::new(Session::new()),
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
