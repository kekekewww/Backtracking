use backtracking_core::{Capture, DurationChoice, WaveformView};
use std::{hint::black_box, time::Instant};

fn percentile(samples: &[f64], q: f64) -> f64 {
    let mut s = samples.to_vec();
    s.sort_by(|a,b| a.total_cmp(b));
    s[((s.len()-1) as f64*q).round() as usize]
}
fn report(case: &str, stage: &str, samples: &[f64]) {
    println!("{{\"case\":\"{case}\",\"stage\":\"{stage}\",\"n\":{},\"median_ms\":{:.6},\"p95_ms\":{:.6},\"max_ms\":{:.6}}}", samples.len(), percentile(samples,0.5),percentile(samples,0.95),percentile(samples,1.0));
}
fn elapsed_ms(start: Instant) -> f64 { start.elapsed().as_secs_f64()*1000.0 }
fn one(rate: u32, ab: bool) {
    let case = format!("{}k_{}", rate/1000, if ab {"stereo_ab"} else {"stereo"});
    let start = Instant::now();
    let mut capture = Capture::new_tracks(rate, 2, if ab {Some(2)} else {None}, DurationChoice::Minutes10).unwrap();
    report(&case,"construct",&[elapsed_ms(start)]);
    let frames = (rate/100) as usize;
    let channels = if ab {4} else {2};
    let block = vec![0.05f32; frames*channels];
    let mut cold = Vec::with_capacity(6000);
    let mut later = Vec::with_capacity(6000);
    let start = Instant::now();
    for n in 0..60000 {
        let block_start = Instant::now();
        capture.push_interleaved(black_box(&block)).unwrap();
        let ms = elapsed_ms(block_start);
        if n < 6000 {cold.push(ms);} else if n >= 54000 {later.push(ms);}
    }
    report(&case,"fill_600s_total",&[elapsed_ms(start)]);
    report(&case,"first_60s_append_10ms_block",&cold);
    report(&case,"last_60s_append_10ms_block",&later);
    for bins in [384,1920,4096] {
        let mut low = vec![0f32;bins]; let mut high = low.clone();
        let mut times=Vec::new();
        for _ in 0..20 {
            let start=Instant::now();
            for lane in 0..if ab {2} else {1} {
                for view in if ab {vec![WaveformView::Merged]} else {vec![WaveformView::Left,WaveformView::Right]} {
                    capture.track_waveform(lane,view,&mut low,&mut high);
                    black_box((&low,&high));
                }
            }
            times.push(elapsed_ms(start));
        }
        report(&case,&format!("full_waveform_{bins}_bins_two_traces"),&times);
    }
    let start=Instant::now();
    let snapshot = capture.track_snapshot(0).unwrap();
    black_box(snapshot.samples.first());
    report(&case,"copy_600s_lane_snapshot",&[elapsed_ms(start)]);
    drop(snapshot);
    let start=Instant::now();capture.set_retention(DurationChoice::Minutes3);report(&case,"shrink_600_to_180",&[elapsed_ms(start)]);
    let start=Instant::now();capture.set_retention(DurationChoice::Minutes10);report(&case,"expand_180_to_600",&[elapsed_ms(start)]);
    let start=Instant::now();capture.set_retention(DurationChoice::Seconds30);report(&case,"shrink_180_to_30",&[elapsed_ms(start)]);
    let mut changes=Vec::new();
    for _ in 0..1000 {
        let start=Instant::now();
        capture.set_retention(DurationChoice::Minutes10);
        capture.set_retention(DurationChoice::Seconds30);
        changes.push(elapsed_ms(start));
    }
    report(&case,"warm_expand_shrink_pair",&changes);
    assert_eq!(capture.frame_range().end-capture.frame_range().start, rate as u64*30);
}
fn main() {
    for (rate,ab) in [(48000,false),(96000,false),(48000,true)] {one(rate,ab);}
}
