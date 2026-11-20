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
    assert!(
        std::path::Path::new(&profile)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("s3-3-")
    );
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
            editor.capture(std::path::Path::new(&path));
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

#[cfg(feature = "editor")]
mod editor_window {
    use super::*;
    use windows::{
        Win32::{Foundation::HWND, UI::WindowsAndMessaging::*},
        core::w,
    };

    pub(super) struct Window(HWND, ComPtr<IPlugView>);
    impl Window {
        pub(super) unsafe fn open(component: &ComPtr<IComponent>) -> Self {
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
                assert_eq!(view.attached(parent.0, kPlatformTypeHWND), kResultOk);
                Self(parent, view)
            }
        }
    }
    pub(super) fn pump() {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
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
        pub(super) fn capture(&self, path: &std::path::Path) {
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
