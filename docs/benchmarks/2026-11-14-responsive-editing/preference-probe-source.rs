#![allow(dead_code)]

mod controller {
    use backtracking_core::ChannelRoute;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) enum CaptureSource {
        System,
        DefaultMicrophone,
        Microphone { id: String, name: String },
        Application { path: String, name: String },
        Sender { id: String, name: String },
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct SourceSettings {
        pub(crate) source: CaptureSource,
        pub(crate) route: ChannelRoute,
    }
}

mod preferences {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../desktop/windows/src/preferences.rs"
    ));

    fn summary(samples: &[f64]) -> serde_json::Value {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let percentile = |q: f64| {
            let rank = (sorted.len() - 1) as f64 * q;
            let low = rank.floor() as usize;
            let high = rank.ceil() as usize;
            sorted[low] + (sorted[high] - sorted[low]) * (rank - low as f64)
        };
        serde_json::json!({
            "n": sorted.len(),
            "min_ms": sorted[0],
            "median_ms": percentile(0.5),
            "p95_ms": percentile(0.95),
            "max_ms": sorted[sorted.len() - 1],
            "samples_ms": samples,
        })
    }

    pub(crate) fn run() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        let output = workspace.join("target/preference-perf-probe");
        fs::create_dir_all(&output).unwrap();
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let run = output.join(format!("standalone-run-{}-{sequence}", std::process::id()));
        let mut fresh = Vec::new();
        for index in 0..8 {
            let preferences = Preferences {
                directory: Some(run.join(format!("fresh-{index}"))),
            };
            let started = std::time::Instant::now();
            preferences.save(Key::History, "600").unwrap();
            fresh.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(preferences.load().history, DurationChoice::Minutes10);
        }
        let preferences = Preferences { directory: Some(run.join("existing")) };
        preferences.save(Key::History, "600").unwrap();
        let mut warm = Vec::new();
        for index in 0..64 {
            let started = std::time::Instant::now();
            preferences.save(Key::History, if index % 2 == 0 { "10" } else { "600" }).unwrap();
            warm.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        assert_eq!(preferences.load().history, DurationChoice::Minutes10);
        let result = serde_json::json!({
            "schema_version": 1,
            "scope": "workspace-only actual included Preferences::save; durable sync_all and atomic rename retained",
            "harness": "controller source enums are stand-ins used only for parsing/default Settings; the included production save implementation performs every measured write",
            "limitations": "new process; fresh directories are not cold boot, cache eviction, HDD or antivirus emulation; durations are not GUI latency",
            "build": "Release",
            "target": std::env::consts::ARCH,
            "operating_system": std::env::consts::OS,
            "finished_unix_ms": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
            "value_bytes": [2, 3],
            "quantiles": "linear interpolation between sorted ranks, type 7",
            "fresh_directory_first_save": summary(&fresh),
            "existing_directory_history_replace": summary(&warm),
            "reloaded_final_history_seconds": preferences.load().history.seconds(),
        });
        fs::write(
            output.join("synchronous-save-results.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        ).unwrap();
        println!("{result}");
    }
}

fn main() {
    preferences::run();
}
