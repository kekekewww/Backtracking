use nice_plug::prelude::*;
use std::sync::Arc;

#[derive(Params, Default)]
struct EmptyParams {}

#[derive(Default)]
struct Probe;

const MONO: &[NonZeroU32] = &[new_nonzero_u32(1)];
const STEREO: &[NonZeroU32] = &[new_nonzero_u32(2)];

const fn layout(main: u32, sidechain: &'static [NonZeroU32]) -> AudioIOLayout {
    AudioIOLayout {
        main_input_channels: NonZeroU32::new(main),
        main_output_channels: NonZeroU32::new(main),
        aux_input_ports: sidechain,
        ..AudioIOLayout::const_default()
    }
}

impl Plugin for Probe {
    const NAME: &'static str = "Surround Probe";
    const VENDOR: &'static str = "Backtracking";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0.1";
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        layout(12, &[]),
        layout(12, MONO),
        layout(12, STEREO),
        layout(10, &[]),
        layout(10, MONO),
        layout(10, STEREO),
        layout(2, &[]),
        layout(2, MONO),
        layout(2, STEREO),
        layout(1, &[]),
        layout(1, MONO),
        layout(1, STEREO),
    ];
    type Editor = ();
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        Arc::new(EmptyParams {})
    }

    fn activate(
        &mut self,
        layout: &AudioIOLayout,
        _: &BufferConfig,
        context: &mut impl ActivateContext<Self>,
    ) -> bool {
        let bits = main_input_speaker_bits(context);
        let valid = bits
            .as_ref()
            .is_some_and(|bits| bits.len() == layout.main_input_channels.unwrap().get() as usize);
        #[cfg(test)]
        tests::OBSERVED.set(bits);
        valid
    }

    fn process(
        &mut self,
        _: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for Probe {
    const VST3_CLASS_ID: [u8; 16] = *b"SurroundBusProbe";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

nice_export_vst3!(Probe);

#[cfg(test)]
#[path = "../tests/surround.rs"]
mod tests;
