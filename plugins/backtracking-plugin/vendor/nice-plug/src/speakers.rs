use std::cell::Cell;

use crate::prelude::{ActivateContext, Plugin, PluginApi};

thread_local! {
    static MAIN_BUSES: Cell<(Option<u64>, Option<u64>)> = const { Cell::new((None, None)) };
}

/// Main input speaker bit positions in buffer channel order (ascending bit).
/// Call on the activation thread inside `Plugin::activate`. CLAP, standalone,
/// a missing main input, and a count without a known layout return `None`.
/// VST3 uses the negotiated mask, or the standard default before negotiation.
///
/// For speaker layouts, bits 0 through 19 are L, R, C, LFE, Ls, Rs, Lc, Rc,
/// Cs, Sl, Sr, Tc, Tfl, Tfc, Tfr, Trl, Trc, Trr, LFE2 and Mono.
/// Ls/Rs are the back pair whenever Sl/Sr are also present, as in 7.1.4.
/// Bits 24 through 37 are Tsl, Tsr, Lcs, Rcs, Bfl, Bfc, Bfr, Pl, Pr, Bsl,
/// Bsr, Brl, Brc and Brr. Bits 59 and 60 are left and right wide.
/// Other set bits also retain their channel positions. Ambisonic arrangements
/// describe components in ACN order instead of physical speakers.
pub fn main_input_speaker_bits<P: Plugin>(context: &impl ActivateContext<P>) -> Option<Vec<u32>> {
    speaker_bits(context, MAIN_BUSES.get().0)
}

/// Main output speaker bit positions in buffer channel order during activation.
/// VST3 supplies the negotiated main output arrangement or the standard default
/// for its width. Returns `None` for other APIs, no main output or no known layout.
/// See [`main_input_speaker_bits`] for speaker bit assignments.
pub fn main_output_speaker_bits<P: Plugin>(context: &impl ActivateContext<P>) -> Option<Vec<u32>> {
    speaker_bits(context, MAIN_BUSES.get().1)
}

fn speaker_bits<P: Plugin>(
    context: &impl ActivateContext<P>,
    mask: Option<u64>,
) -> Option<Vec<u32>> {
    if context.plugin_api() != PluginApi::Vst3 {
        return None;
    }
    mask.map(|mask| (0..64).filter(|bit| mask & (1u64 << bit) != 0).collect())
}

#[cfg(feature = "vst3")]
pub(crate) fn with_main_buses<T>(
    input: Option<u64>,
    output: Option<u64>,
    activate: impl FnOnce() -> T,
) -> T {
    struct Restore((Option<u64>, Option<u64>));
    impl Drop for Restore {
        fn drop(&mut self) {
            MAIN_BUSES.set(self.0);
        }
    }
    let _restore = Restore(MAIN_BUSES.replace((input, output)));
    activate()
}
