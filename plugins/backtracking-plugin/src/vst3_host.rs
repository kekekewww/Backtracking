//! Drives the shipped DLL with memory buffers. No audio device is opened.
use backtracking_core::SenderReceiver;
use std::{
    cell::RefCell,
    ffi::c_void,
    io::{Cursor, Read, Seek, SeekFrom},
    ptr,
    time::{Duration, Instant},
};
use vst3::{
    Class, ComPtr, ComWrapper, Interface,
    Steinberg::{
        Vst::{SpeakerArr::*, *},
        *,
    },
};

const ID: &str = "c3c1-5e7d-0012";
const FRAMES: usize = 512;

// A saved project state enabling this instance's Sender.
struct State(RefCell<Cursor<&'static [u8]>>);
impl Class for State {
    type Interfaces = (IBStream,);
}
impl IBStreamTrait for State {
    unsafe fn read(&self, data: *mut c_void, bytes: i32, actual: *mut i32) -> tresult {
        if data.is_null() || bytes < 0 {
            return kInvalidArgument;
        }
        let buf = unsafe { std::slice::from_raw_parts_mut(data.cast(), bytes as usize) };
        let n = self.0.borrow_mut().read(buf).unwrap();
        if !actual.is_null() {
            unsafe {
                *actual = n as i32;
            }
        }
        kResultOk
    }
    unsafe fn write(&self, _: *mut c_void, _: i32, _: *mut i32) -> tresult {
        kNotImplemented
    }
    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> tresult {
        let from = match mode {
            0 if pos >= 0 => SeekFrom::Start(pos as u64),
            1 => SeekFrom::Current(pos),
            2 => SeekFrom::End(pos),
            _ => return kInvalidArgument,
        };
        match self.0.borrow_mut().seek(from) {
            Ok(n) => {
                if !result.is_null() {
                    unsafe {
                        *result = n as i64;
                    }
                }
                kResultOk
            }
            Err(_) => kInvalidArgument,
        }
    }
    unsafe fn tell(&self, pos: *mut i64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        unsafe {
            *pos = self.0.borrow().position() as i64;
        }
        kResultOk
    }
}

fn bus(pointers: &mut [*mut f32]) -> AudioBusBuffers {
    AudioBusBuffers {
        numChannels: pointers.len() as i32,
        silenceFlags: 0,
        __field0: AudioBusBuffers__type0 {
            channelBuffers32: pointers.as_mut_ptr(),
        },
    }
}

fn exercise(
    component: &ComPtr<IComponent>,
    processor: &ComPtr<IAudioProcessor>,
    mut mask: u64,
    names: &str,
    hold: Duration,
) {
    let mut out_mask = mask;
    unsafe {
        assert_eq!(
            processor.setBusArrangements(&mut mask, 1, &mut out_mask, 1),
            kResultOk
        );
        assert_eq!(component.setActive(1), kResultOk);
        assert_eq!(processor.setProcessing(1), kResultOk);
    }
    let width = mask.count_ones() as usize;
    let mut tones: Vec<Vec<f32>> = (0..width)
        .map(|ch| {
            (0..FRAMES)
                .map(|i| {
                    (0.6 * ((i as f64 + 0.125) * (ch + 1) as f64 * std::f64::consts::TAU
                        / FRAMES as f64)
                        .sin()) as f32
                })
                .collect()
        })
        .collect();
    let expected: Vec<_> = (0..FRAMES)
        .flat_map(|i| tones.iter().map(move |ch| ch[i].to_bits()))
        .collect();
    let mut output = vec![vec![0.; FRAMES]; width];
    let mut inputs: Vec<_> = tones.iter_mut().map(Vec::as_mut_ptr).collect();
    let mut outputs: Vec<_> = output.iter_mut().map(Vec::as_mut_ptr).collect();
    let mut input_bus = bus(&mut inputs);
    let mut output_bus = bus(&mut outputs);
    let mut data = ProcessData {
        numSamples: FRAMES as i32,
        numInputs: 1,
        numOutputs: 1,
        inputs: &mut input_bus,
        outputs: &mut output_bus,
        ..unsafe { std::mem::zeroed() }
    };
    let start = Instant::now();
    let mut next = start;
    let mut receiver = None;
    let mut samples = Vec::new();
    let mut packets = 0;
    loop {
        for channel in &mut output {
            channel.fill(f32::NAN);
        }
        assert_eq!(unsafe { processor.process(&mut data) }, kResultOk);
        for (ch, out) in output.iter().enumerate() {
            assert!(
                out.iter()
                    .enumerate()
                    .all(|(i, s)| s.to_bits() == expected[i * width + ch])
            );
        }
        if receiver.is_none()
            && let Ok(r) = SenderReceiver::connect(ID)
            && r.labels().is_some_and(|l| l.join(" ") == names)
        {
            receiver = Some(r);
        }
        if let Some(r) = &mut receiver {
            while let Some(packet) = r.packet(&mut samples, &mut Vec::new()).unwrap() {
                assert_eq!(packet.sample_rate, 48000);
                assert_eq!(packet.frames, FRAMES);
                assert!(
                    samples
                        .iter()
                        .map(|s| s.to_bits())
                        .eq(expected.iter().copied())
                );
                packets += 1;
            }
        }
        if packets >= 3 && start.elapsed() >= hold {
            break;
        }
        assert!(
            start.elapsed() < hold + Duration::from_secs(5),
            "no matching Sender packets for {names}"
        );
        #[cfg(feature = "editor")]
        if std::env::var_os("S3_3_EDITOR_PNG").is_some() {
            editor_window::pump();
        }
        next += Duration::from_secs_f64(FRAMES as f64 / 48000.);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    println!("{width} channels: {packets} verified packets, {names}");
    unsafe {
        assert_eq!(processor.setProcessing(0), kResultOk);
        assert_eq!(component.setActive(0), kResultOk);
    }
}

#[test]
#[ignore = "set BACKTRACKING_TEST_VST3 to the release DLL and LOCALAPPDATA to a test profile"]
fn process_surround_dll() {
    let path = std::env::var_os("BACKTRACKING_TEST_VST3").unwrap();
    let profile = std::env::var("LOCALAPPDATA").unwrap();
    assert!(test_profile(std::path::Path::new(&profile)));
    unsafe {
        let module = libloading::Library::new(path).unwrap();
        let factory = module
            .get::<extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
            .unwrap();
        let factory = ComPtr::from_raw(factory()).unwrap();
        let mut info: PClassInfo = std::mem::zeroed();
        assert_eq!(factory.getClassInfo(0, &mut info), kResultOk);
        let mut raw = ptr::null_mut();
        assert_eq!(
            factory.createInstance(info.cid.as_ptr(), IComponent::IID.as_ptr().cast(), &mut raw),
            kResultOk
        );
        let component = ComPtr::<IComponent>::from_raw(raw.cast()).unwrap();
        assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
        let processor = component.cast::<IAudioProcessor>().unwrap();
        let bytes=br#"{"params":{},"fields":{"capture_settings_v1":"{\"sender_enabled\":\"true\",\"sender_instance_id\":\"c3c1-5e7d-0012\",\"sender_name\":\"S3-3 plug-in\"}"}}"#;
        let state = ComWrapper::new(State(RefCell::new(Cursor::new(bytes))));
        assert_eq!(
            component.setState(state.as_com_ref::<IBStream>().unwrap().as_ptr()),
            kResultOk
        );
        let mut setup = ProcessSetup {
            processMode: 0,
            symbolicSampleSize: 0,
            maxSamplesPerBlock: FRAMES as i32,
            sampleRate: 48000.,
        };
        assert_eq!(processor.setupProcessing(&mut setup), kResultOk);
        let hold = std::env::var("S3_3_HOLD_SECONDS")
            .ok()
            .map_or(0, |s| s.parse().unwrap());
        #[cfg(feature = "editor")]
        let editor = std::env::var_os("S3_3_EDITOR_PNG")
            .map(|path| (editor_window::Window::open(&component), path));
        exercise(
            &component,
            &processor,
            k71_4,
            "FL FR FC LFE Lb Rb Ls Rs TFL TFR TBL TBR",
            Duration::from_secs(hold),
        );
        #[cfg(feature = "editor")]
        if let Some((editor, path)) = editor {
            let _ = editor.capture(std::path::Path::new(&path));
        }
        exercise(
            &component,
            &processor,
            k71_2,
            "FL FR FC LFE Lb Rb Ls Rs Ltm Rtm",
            Duration::ZERO,
        );
        exercise(
            &component,
            &processor,
            k51_4,
            "FL FR FC LFE Ls Rs TFL TFR TBL TBR",
            Duration::ZERO,
        );
        assert_eq!(component.terminate(), kResultOk);
    }
}

/// Whether `profile` is a test profile folder, which the host may use as LOCALAPPDATA.
fn test_profile(profile: &std::path::Path) -> bool {
    let name = profile.file_name().unwrap_or_default().to_string_lossy();
    name.starts_with("s3-3-")
        || name.starts_with("s3-4b-")
        || (name.starts_with("s3-5-")
            && profile
                .canonicalize()
                .ok()
                .zip(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("target")
                        .canonicalize()
                        .ok(),
                )
                .is_some_and(|(path, target)| path.starts_with(target)))
}

/// One step of a scripted editor run, from `S3_4B_STEPS`. Points are in the editor's pixels.
#[cfg(feature = "editor")]
#[derive(Debug)]
enum Step {
    /// Save the editor's last displayed frame as this PNG, in `S3_4B_OUT`.
    Png(String),
    Silence,
    Space,
    DragStart,
    DragTo(i32, i32),
    DragEnd,
    CheckPreview {
        native: bool,
    },
    Move(i32, i32),
    /// A press and release of the left (`false`) or right (`true`) button where the pointer is.
    Press(bool),
    /// Turns of the wheel where the pointer is; negative turns scroll down.
    Wheel(i32),
    /// The pointer leaves the editor.
    Leave,
    Wait(u64),
    /// The snapshot shortcut, as the desktop sends it to the Sender.
    Snapshot,
    /// Waits until the saved folder has WAV files and stops changing.
    Saved,
    /// The host gives the editor this size, as when the user resizes the plug-in window.
    Size(i32, i32),
    /// The host puts the editor's window back to this size itself, as a host that keeps plug-in
    /// windows at the size they opened with. The editor cannot be resized by a host otherwise.
    Clamp(i32, i32),
}

#[cfg(feature = "editor")]
fn steps(script: &str) -> std::collections::VecDeque<Step> {
    let point = |text: &str| {
        let mut parts = text
            .split(',')
            .map(|part| part.trim().parse::<i32>().unwrap());
        (parts.next().unwrap(), parts.next().unwrap(), parts.next())
    };
    let mut steps = std::collections::VecDeque::new();
    for item in script
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let (name, argument) = item.split_once(':').unwrap_or((item, ""));
        match name {
            "silence" => steps.push_back(Step::Silence),
            "space" => steps.push_back(Step::Space),
            "check" => steps.push_back(Step::CheckPreview {
                native: match argument {
                    "own" => true,
                    "lr" => false,
                    _ => panic!("expected check:own or check:lr"),
                },
            }),
            "drag" => {
                let xy: Vec<i32> = argument
                    .split(',')
                    .map(|v| v.trim().parse().unwrap())
                    .collect();
                let [x, y, end_x, end_y] = xy[..] else {
                    panic!("expected drag:x,y,end_x,end_y")
                };
                steps.extend([
                    Step::Move(x, y),
                    Step::Wait(150),
                    Step::DragStart,
                    Step::Wait(200),
                    Step::DragTo(end_x, end_y),
                    Step::Wait(200),
                    Step::DragEnd,
                    Step::Wait(400),
                ]);
            }
            "png" => steps.push_back(Step::Png(argument.to_owned())),
            "move" | "click" | "rclick" | "wheel" => {
                let (x, y, turns) = point(argument);
                steps.extend([Step::Move(x, y), Step::Wait(150)]);
                steps.push_back(match name {
                    "click" => Step::Press(false),
                    "rclick" => Step::Press(true),
                    "wheel" => Step::Wheel(turns.unwrap()),
                    _ => Step::Wait(0),
                });
                steps.push_back(Step::Wait(400));
            }
            "leave" => steps.extend([Step::Leave, Step::Wait(400)]),
            "wait" => steps.push_back(Step::Wait(argument.parse().unwrap())),
            "size" | "clamp" => {
                let (width, height, _) = point(argument);
                steps.push_back(if name == "size" {
                    Step::Size(width, height)
                } else {
                    Step::Clamp(width, height)
                });
                steps.push_back(Step::Wait(600));
            }
            "snapshot" => steps.push_back(Step::Snapshot),
            "saved" => steps.push_back(Step::Saved),
            _ => panic!("unknown step {item}"),
        }
    }
    steps
}

/// The WAV files under `folder` and their total size.
#[cfg(feature = "editor")]
fn saved_files(folder: &std::path::Path) -> (usize, u64) {
    let mut found = (0, 0);
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            let (count, bytes) = saved_files(&path);
            found = (found.0 + count, found.1 + bytes);
        } else if path.extension().is_some_and(|extension| extension == "wav") {
            found = (
                found.0 + 1,
                found.1 + entry.metadata().map_or(0, |data| data.len()),
            );
        }
    }
    found
}

/// Each input tone completes an integer number of periods in one host block.
#[cfg(feature = "editor")]
fn check_preview(output: &[Vec<f32>], native: bool) {
    assert!(output.len() > 2);
    for (channel, samples) in output.iter().enumerate() {
        assert!(samples.iter().all(|x| x.is_finite()));
        if native {
            for tone in 0..output.len() {
                let mut re = 0.0;
                let mut im = 0.0;
                for (frame, &sample) in samples.iter().enumerate() {
                    let phase = std::f64::consts::TAU * ((tone + 1) * frame) as f64 / FRAMES as f64;
                    let (sin, cos) = phase.sin_cos();
                    re += f64::from(sample) * cos;
                    im += f64::from(sample) * sin;
                }
                let amplitude = 2.0 * re.hypot(im) / FRAMES as f64;
                let expected = if tone == channel {
                    0.6 - 0.035 * channel as f64
                } else {
                    0.0
                };
                assert!(
                    (amplitude - expected).abs() < 0.0001,
                    "output {channel}, tone {tone}: {amplitude}, expected {expected}"
                );
            }
        } else if channel < 2 {
            assert!(
                samples.iter().any(|x| x.abs() > 0.01),
                "silent pair channel {channel}"
            );
        } else {
            assert!(
                samples.iter().all(|&x| x == 0.0),
                "unexpected output on {channel}"
            );
        }
    }
}

/// Drives the shipped DLL's editor through one scenario for pictures and saves. The scenario
/// comes from the environment: `S3_4B_LAYOUT` (`7.1.4` by default, `stereo`, or `7.1.2-to-5.1.4`),
/// `S3_4B_SIDECHAIN` (any value adds a stereo sidechain bus and captures both),
/// `S3_4B_FIELDS` (`key=value;...` added to the saved settings), `S3_4B_FRAME` (`0` for a host
/// that refuses to resize the editor), `S3_4B_HOLD_SECONDS` (audio before the first step),
/// `S3_4B_STEPS` (see [`steps`]) and `S3_4B_OUT` (the folder for pictures). Saves go to the
/// profile's `saved` folder. Every channel gets its own tone, quieter as the channel number
/// rises. Before `silence`, each block must match the input. Use `drag:x,y,end_x,end_y`,
/// `space` and then `check:own` or `check:lr` on a steady preview block.
#[cfg(feature = "editor")]
#[test]
#[ignore = "set BACKTRACKING_TEST_VST3 to the release DLL, LOCALAPPDATA to a test profile and S3_4B_* to the scenario"]
fn editor_pictures_dll() {
    use nice_plug::params::persist::serialize_field;
    use std::collections::BTreeMap;
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let path = var("BACKTRACKING_TEST_VST3").unwrap();
    let profile = std::path::PathBuf::from(var("LOCALAPPDATA").unwrap());
    assert!(test_profile(&profile));
    let out = var("S3_4B_OUT").map_or_else(|| profile.clone(), std::path::PathBuf::from);
    let (main, mut output_layout) = match var("S3_4B_LAYOUT").as_deref() {
        Some("stereo") => (kStereo, kStereo),
        Some("7.1.2-to-5.1.4") => (k71_2, k51_4),
        _ => (k71_4, k71_4),
    };
    let sidechain = var("S3_4B_SIDECHAIN").is_some();
    let saved = profile.join("saved");
    let mut fields = BTreeMap::from([
        ("sender_enabled".to_owned(), "true".to_owned()),
        ("sender_instance_id".to_owned(), ID.to_owned()),
        ("sender_name".to_owned(), "S3-4b plug-in".to_owned()),
        (
            "saved_dir".to_owned(),
            serialize_field(saved.as_os_str()).unwrap(),
        ),
        (
            "output_dir".to_owned(),
            serialize_field(profile.join("output").as_os_str()).unwrap(),
        ),
    ]);
    if sidechain {
        fields.insert("capture_input".to_owned(), "both".to_owned());
    }
    for field in var("S3_4B_FIELDS").unwrap_or_default().split(';') {
        if let Some((key, value)) = field.split_once('=') {
            fields.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    let state = format!(
        r#"{{"params":{{}},"fields":{{"capture_settings_v1":{}}}}}"#,
        serialize_field(&serialize_field(&fields).unwrap()).unwrap()
    );
    let state: &'static [u8] = Box::leak(state.into_bytes().into_boxed_slice());
    let mut script = steps(&var("S3_4B_STEPS").unwrap_or_default());
    let hold =
        Duration::from_secs_f64(var("S3_4B_HOLD_SECONDS").map_or(0.0, |s| s.parse().unwrap()));
    let resizable = var("S3_4B_FRAME").as_deref() != Some("0");
    unsafe {
        let module = libloading::Library::new(path).unwrap();
        let factory = module
            .get::<extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
            .unwrap();
        let factory = ComPtr::from_raw(factory()).unwrap();
        let mut info: PClassInfo = std::mem::zeroed();
        assert_eq!(factory.getClassInfo(0, &mut info), kResultOk);
        let mut raw = ptr::null_mut();
        assert_eq!(
            factory.createInstance(info.cid.as_ptr(), IComponent::IID.as_ptr().cast(), &mut raw),
            kResultOk
        );
        let component = ComPtr::<IComponent>::from_raw(raw.cast()).unwrap();
        assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
        let processor = component.cast::<IAudioProcessor>().unwrap();
        let state = ComWrapper::new(State(RefCell::new(Cursor::new(state))));
        assert_eq!(
            component.setState(state.as_com_ref::<IBStream>().unwrap().as_ptr()),
            kResultOk
        );
        let mut setup = ProcessSetup {
            processMode: 0,
            symbolicSampleSize: 0,
            maxSamplesPerBlock: FRAMES as i32,
            sampleRate: 48000.,
        };
        assert_eq!(processor.setupProcessing(&mut setup), kResultOk);
        let window = editor_window::Window::open_framed(&component, resizable);
        let mut inputs = [main, kStereo];
        let buses = 1 + i32::from(sidechain);
        assert_eq!(
            processor.setBusArrangements(inputs.as_mut_ptr(), buses, &mut output_layout, 1),
            kResultOk
        );
        assert_eq!(component.setActive(1), kResultOk);
        assert_eq!(processor.setProcessing(1), kResultOk);
        let width = main.count_ones() as usize;
        let channels = width + if sidechain { 2 } else { 0 };
        let mut tones: Vec<Vec<f32>> = (0..channels)
            .map(|ch| {
                let amplitude = 0.6 - 0.035 * ch as f64;
                (0..FRAMES)
                    .map(|i| {
                        (amplitude
                            * ((i as f64 + 0.125) * (ch + 1) as f64 * std::f64::consts::TAU
                                / FRAMES as f64)
                                .sin()) as f32
                    })
                    .collect()
            })
            .collect();
        let expected: Vec<Vec<u32>> = tones[..width]
            .iter()
            .map(|tone| tone.iter().map(|s| s.to_bits()).collect())
            .collect();
        let mut output = vec![vec![0.; FRAMES]; width];
        let (main_tones, aux_tones) = tones.split_at_mut(width);
        let mut main_pointers: Vec<_> = main_tones.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut aux_pointers: Vec<_> = aux_tones.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut outputs: Vec<_> = output.iter_mut().map(Vec::as_mut_ptr).collect();
        let mut input_buses = [bus(&mut main_pointers), bus(&mut aux_pointers)];
        let mut output_bus = bus(&mut outputs);
        let mut data = ProcessData {
            numSamples: FRAMES as i32,
            numInputs: buses,
            numOutputs: 1,
            inputs: input_buses.as_mut_ptr(),
            outputs: &mut output_bus,
            ..std::mem::zeroed()
        };
        let start = Instant::now();
        let mut next = start;
        let mut ready = start + hold;
        let mut silent_input = false;
        let mut settled = (0, 0, start);
        loop {
            for channel in &mut output {
                channel.fill(f32::NAN);
            }
            assert_eq!(processor.process(&mut data), kResultOk);
            if !silent_input {
                for (out, want) in output.iter().zip(&expected) {
                    assert!(out.iter().map(|s| s.to_bits()).eq(want.iter().copied()));
                }
            }
            window.pump();
            let now = Instant::now();
            if now >= ready {
                let Some(step) = script.pop_front() else {
                    break;
                };
                ready = now;
                match &step {
                    Step::Silence => {
                        for tone in &mut tones {
                            tone.fill(0.0);
                        }
                        silent_input = true;
                    }
                    Step::Space => {
                        window.send(0x0100, 0x20, 1, 0x39);
                        window.send(0x0101, 0x20, 1, 0xc039);
                    }
                    Step::DragStart => window.send(0x0201, 9, -1, -1),
                    Step::DragTo(x, y) => window.send(0x0200, 9, *x, *y),
                    Step::DragEnd => window.send(0x0202, 0, -1, -1),
                    Step::CheckPreview { native } => {
                        assert!(silent_input);
                        check_preview(&output, *native);
                        println!("preview verified on silent input: {step:?}");
                    }
                    Step::Png(name) => {
                        let size = window.capture(&out.join(name));
                        println!("{name}: {} x {}", size.0, size.1);
                    }
                    Step::Move(x, y) => window.send(0x0200, 0, *x, *y),
                    Step::Press(right) => {
                        let (down, up, held) = if *right {
                            (0x0204, 0x0205, 2)
                        } else {
                            (0x0201, 0x0202, 1)
                        };
                        // Back to back, so the editor holds the pointer capture for no time.
                        window.send(down, held, -1, -1);
                        window.send(up, 0, -1, -1);
                    }
                    Step::Wheel(turns) => {
                        window.send(0x020A, ((turns * 120) as i16 as u16 as usize) << 16, -1, -1)
                    }
                    Step::Leave => window.send(0x02A3, 0, -1, -1),
                    Step::Size(width, height) => window.resize(*width, *height),
                    Step::Clamp(width, height) => window.clamp(*width, *height),
                    Step::Wait(ms) => ready = now + Duration::from_millis(*ms),
                    Step::Snapshot => {
                        if backtracking_core::request_sender_snapshot(ID).is_err() {
                            ready = now + Duration::from_millis(100);
                            script.push_front(step);
                        }
                    }
                    Step::Saved => {
                        let (count, bytes) = saved_files(&saved);
                        if (count, bytes) != (settled.0, settled.1) {
                            settled = (count, bytes, now);
                        }
                        if count == 0 || now < settled.2 + Duration::from_millis(1500) {
                            assert!(
                                now < start + hold + Duration::from_secs(120),
                                "no saved files"
                            );
                            ready = now + Duration::from_millis(250);
                            script.push_front(step);
                        } else {
                            println!("saved {count} files, {bytes} bytes");
                        }
                    }
                }
            }
            next += Duration::from_secs_f64(FRAMES as f64 / 48000.);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        assert_eq!(processor.setProcessing(0), kResultOk);
        assert_eq!(component.setActive(0), kResultOk);
        drop(window);
        assert_eq!(component.terminate(), kResultOk);
    }
}

#[cfg(feature = "editor")]
mod editor_window {
    use super::*;
    use windows::{
        Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
        core::w,
    };

    /// A host frame that resizes the editor when it asks, on the next pump, as hosts do.
    pub(super) struct Frame(std::sync::Mutex<Option<ViewRect>>);
    impl Class for Frame {
        type Interfaces = (IPlugFrame,);
    }
    impl IPlugFrameTrait for Frame {
        unsafe fn resizeView(&self, _view: *mut IPlugView, size: *mut ViewRect) -> tresult {
            if size.is_null() {
                return kInvalidArgument;
            }
            *self.0.lock().unwrap() = Some(unsafe { *size });
            kResultOk
        }
    }

    pub(super) struct Window(HWND, ComPtr<IPlugView>, Option<ComWrapper<Frame>>);
    impl Window {
        pub(super) unsafe fn open(component: &ComPtr<IComponent>) -> Self {
            unsafe { Self::open_framed(component, false) }
        }

        /// Opens the editor; with `resizable`, under a frame that grants its size requests.
        pub(super) unsafe fn open_framed(component: &ComPtr<IComponent>, resizable: bool) -> Self {
            unsafe {
                let controller = component.cast::<IEditController>().unwrap();
                let view = ComPtr::from_raw(controller.createView(ViewType::kEditor)).unwrap();
                let mut rect: ViewRect = std::mem::zeroed();
                assert_eq!(view.getSize(&mut rect), kResultOk);
                let parent = CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("S3-3 test editor"),
                    WS_POPUP | WS_VISIBLE,
                    20000,
                    20000,
                    rect.right,
                    rect.bottom,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();
                let frame = resizable.then(|| ComWrapper::new(Frame(Default::default())));
                if let Some(frame) = &frame {
                    let frame = frame.as_com_ref::<IPlugFrame>().unwrap();
                    assert_eq!(view.setFrame(frame.as_ptr()), kResultOk);
                }
                assert_eq!(view.attached(parent.0, kPlatformTypeHWND), kResultOk);
                Self(parent, view, frame)
            }
        }

        /// Handles the editor's messages, and after each one a size it asked the frame for, as
        /// a host that resizes at once. The real pointer is elsewhere, so Windows says at once
        /// that a pointer moved here left; that message is dropped and the pointer stays until a
        /// `Leave` step.
        pub(super) fn pump(&self) {
            let mut msg = MSG::default();
            let mut paints = 0;
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    count_paint(&msg, &mut paints);
                    if msg.message != 0x02A3 {
                        dispatch(&msg);
                    }
                    let requested = self
                        .2
                        .as_ref()
                        .and_then(|frame| frame.0.lock().unwrap().take());
                    if let Some(rect) = requested {
                        self.resize(rect.right - rect.left, rect.bottom - rect.top);
                    }
                }
            }
        }

        /// Gives the editor `width` x `height`, as a host does when its window is resized.
        pub(super) fn resize(&self, width: i32, height: i32) {
            let mut rect = ViewRect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            unsafe {
                let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
                SetWindowPos(self.0, None, 0, 0, width, height, flags).unwrap();
                assert_eq!(self.1.onSize(&mut rect), kResultOk);
            }
        }

        /// Sets the parent and the editor's own window to `width` x `height`.
        pub(super) fn clamp(&self, width: i32, height: i32) {
            unsafe {
                let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
                SetWindowPos(self.0, None, 0, 0, width, height, flags).unwrap();
                let child = GetWindow(self.0, GW_CHILD).unwrap();
                SetWindowPos(child, None, 0, 0, width, height, flags).unwrap();
            }
        }

        /// Sends a mouse message to the editor at `x`, `y`, or where the pointer is for -1.
        pub(super) fn send(&self, message: u32, wparam: usize, x: i32, y: i32) {
            unsafe {
                let child = GetWindow(self.0, GW_CHILD).unwrap();
                let lparam = ((y as u16 as u32) << 16 | x as u16 as u32) as isize;
                let lparam = (x >= 0).then_some(windows::Win32::Foundation::LPARAM(lparam));
                SendMessageW(
                    child,
                    message,
                    Some(windows::Win32::Foundation::WPARAM(wparam)),
                    lparam,
                );
            }
        }
    }
    pub(super) fn pump() {
        let mut msg = MSG::default();
        let mut paints = 0;
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                count_paint(&msg, &mut paints);
                dispatch(&msg);
            }
        }
    }

    /// Fails when WM_PAINT keeps coming within one pump. Windows sends it for as long as part of
    /// a window's update region stays invalid, and it comes before WM_TIMER, so an editor that
    /// leaves part unvalidated (as one grown past a parent the host has not resized did) never
    /// redraws again while the host goes on. In Nuendo that froze the editor.
    fn count_paint(msg: &MSG, paints: &mut u32) {
        if msg.message == WM_PAINT {
            *paints += 1;
            assert!(
                *paints < 1000,
                "WM_PAINT keeps coming: the editor leaves part of its update region invalid"
            );
        }
    }

    unsafe fn dispatch(msg: &MSG) {
        unsafe {
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetDC(window: *mut c_void) -> *mut c_void;
        fn ReleaseDC(window: *mut c_void, dc: *mut c_void) -> i32;
    }
    #[link(name = "opengl32")]
    unsafe extern "system" {
        fn wglCreateContext(dc: *mut c_void) -> *mut c_void;
        fn wglMakeCurrent(dc: *mut c_void, ctx: *mut c_void) -> i32;
        fn wglDeleteContext(ctx: *mut c_void) -> i32;
        fn glReadBuffer(buffer: u32);
        fn glReadPixels(x: i32, y: i32, w: i32, h: i32, format: u32, kind: u32, data: *mut c_void);
    }

    impl Window {
        pub(super) fn capture(&self, path: &std::path::Path) -> (i32, i32) {
            unsafe {
                let child = GetWindow(self.0, GW_CHILD).unwrap();
                let mut rect = windows::Win32::Foundation::RECT::default();
                GetClientRect(child, &mut rect).unwrap();
                let (w, h) = (rect.right, rect.bottom);
                let dc = GetDC(child.0);
                let ctx = wglCreateContext(dc);
                assert!(!ctx.is_null());
                assert_ne!(wglMakeCurrent(dc, ctx), 0);
                let mut pixels = vec![0u8; w as usize * h as usize * 4];
                // Read the last displayed OpenGL frame, including when the window is offscreen.
                glReadBuffer(0x0404); // GL_FRONT
                glReadPixels(0, 0, w, h, 0x1908, 0x1401, pixels.as_mut_ptr().cast());
                let oriented: Vec<_> = pixels
                    .chunks_exact(w as usize * 4)
                    .rev()
                    .flat_map(|row| {
                        row.as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|rgba| [rgba[0], rgba[1], rgba[2]])
                    })
                    .collect();
                let mut png =
                    png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
                png.set_color(png::ColorType::Rgb);
                png.write_header()
                    .unwrap()
                    .write_image_data(&oriented)
                    .unwrap();
                wglMakeCurrent(ptr::null_mut(), ptr::null_mut());
                wglDeleteContext(ctx);
                ReleaseDC(child.0, dc);
                (w, h)
            }
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            unsafe {
                self.1.removed();
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
