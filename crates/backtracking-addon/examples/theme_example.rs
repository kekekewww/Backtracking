//! An appearance-only add-on with a slowly changing border color and a gradient for coloring
//! the waveform by frequency.

use backtracking_addon::client::Client;
use backtracking_addon::protocol::{Appearance, HostMessage, SpectrumStop};
use backtracking_addon::Result;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const ID: &str = "com.example.theme";
const VERSION: &str = "0.1.0";
const INTERVAL: Duration = Duration::from_millis(250);

fn main() -> Result<()> {
    let mut client = Client::connect_from_env(ID, VERSION)?;
    let mut appearance = Appearance {
        colors: BTreeMap::from([
            ("waveform".into(), [103, 231, 181, 255]),
            ("plot_background".into(), [20, 29, 42, 255]),
            ("border".into(), [88, 120, 168, 224]),
        ]),
        numbers: BTreeMap::from([("border_width".into(), 2.0)]),
        rows: BTreeMap::from([("LFE".into(), [255, 160, 88, 255])]),
        // Used only when the user colors the waveform by frequency and has no gradient of
        // their own: orange for low frequencies, through yellow, to sky blue for high ones.
        spectrum: vec![
            SpectrumStop {
                color: [255, 140, 60, 255],
                position: 0.0,
            },
            SpectrumStop {
                color: [240, 220, 90, 255],
                position: 0.45,
            },
            SpectrumStop {
                color: [110, 200, 255, 255],
                position: 1.0,
            },
        ],
    };
    client.send_appearance(&appearance)?;

    let mut steps = (1..=16).chain((0..16).rev()).cycle();
    let mut next = Instant::now() + INTERVAL;
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        if matches!(client.recv_timeout(wait)?, Some(HostMessage::Quit)) || client.is_disconnected()
        {
            return Ok(());
        }
        if Instant::now() >= next {
            let step = steps.next().unwrap();
            *appearance.colors.get_mut("border").unwrap() = [88, 120 + 2 * step, 168 + step, 224];
            client.send_appearance(&appearance)?;
            next = Instant::now() + INTERVAL;
        }
    }
}
