use std::{cell::Cell, ffi::c_void, ptr};

use ::vst3::Steinberg::Vst::{
    BusDirections_::{kInput, kOutput},
    IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentTrait, ProcessSetup,
    SpeakerArr::*,
    SpeakerArrangement,
};
use ::vst3::Steinberg::{
    IPluginBaseTrait, IPluginFactory, IPluginFactoryTrait, PClassInfo, kInvalidArgument,
    kResultFalse, kResultOk,
};
use ::vst3::{ComPtr, Interface};

thread_local! {
    pub(super) static OBSERVED: Cell<Option<Vec<u32>>> = const { Cell::new(None) };
}

struct Instance {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
}

impl Instance {
    unsafe fn from_factory(raw: *mut c_void) -> Self {
        unsafe {
            let factory = ComPtr::<IPluginFactory>::from_raw(raw.cast()).unwrap();
            let mut info: PClassInfo = std::mem::zeroed();
            assert_eq!(factory.getClassInfo(0, &mut info), kResultOk);
            let mut raw_component = ptr::null_mut();
            assert_eq!(
                factory.createInstance(
                    info.cid.as_ptr(),
                    IComponent::IID.as_ptr().cast(),
                    &mut raw_component
                ),
                kResultOk
            );
            let component = ComPtr::<IComponent>::from_raw(raw_component.cast()).unwrap();
            assert_eq!(component.initialize(ptr::null_mut()), kResultOk);
            let processor = component.cast::<IAudioProcessor>().unwrap();
            let mut setup = ProcessSetup {
                processMode: 0,
                symbolicSampleSize: 0,
                maxSamplesPerBlock: 128,
                sampleRate: 48000.0,
            };
            assert_eq!(processor.setupProcessing(&mut setup), kResultOk);
            Self {
                component,
                processor,
            }
        }
    }

    fn set(&self, inputs: &mut [SpeakerArrangement], mut output: SpeakerArrangement) -> i32 {
        unsafe {
            self.processor.setBusArrangements(
                inputs.as_mut_ptr(),
                inputs.len() as i32,
                &mut output,
                1,
            )
        }
    }

    fn get(&self, direction: i32, index: i32) -> Result<SpeakerArrangement, i32> {
        let mut mask = 0;
        match unsafe {
            self.processor
                .getBusArrangement(direction, index, &mut mask)
        } {
            code if code == kResultOk => Ok(mask),
            code => Err(code),
        }
    }

    fn activate(&self) {
        unsafe {
            assert_eq!(self.component.setActive(1), kResultOk);
            assert_eq!(self.component.setActive(0), kResultOk);
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe { self.component.terminate() };
    }
}

const INPUT: i32 = kInput as i32;
const OUTPUT: i32 = kOutput as i32;

#[test]
fn default_and_negotiated_buses_reach_activation_in_channel_order() {
    let instance = unsafe { Instance::from_factory(super::GetPluginFactory()) };
    let atmos = &[0, 1, 2, 3, 4, 5, 9, 10, 12, 14, 15, 17][..];
    assert_eq!(instance.get(INPUT, 0), Ok(k71_4));
    assert_eq!(instance.get(OUTPUT, 0), Ok(k71_4));
    instance.activate();
    assert_eq!(OBSERVED.take().as_deref(), Some(atmos));

    for (main, expected) in [
        (kMono, &[19][..]),
        (kStereo, &[0, 1]),
        (k71_4, atmos),
        (k51_4, &[0, 1, 2, 3, 4, 5, 12, 14, 15, 17]),
        (k71_2, &[0, 1, 2, 3, 4, 5, 9, 10, 24, 25]),
    ] {
        for aux in [Some(kMono), Some(kStereo), None] {
            let mut inputs = vec![main];
            inputs.extend(aux);
            assert_eq!(instance.set(&mut inputs, main), kResultOk);
            assert_eq!(instance.get(INPUT, 0), Ok(main));
            assert_eq!(instance.get(OUTPUT, 0), Ok(main));
            assert_eq!(instance.get(INPUT, 1), aux.ok_or(kInvalidArgument));
            instance.activate();
            assert_eq!(OBSERVED.take().as_deref(), Some(expected));
        }
    }
}

#[test]
fn refusal_preserves_each_bus_and_other_instances() {
    let instance = unsafe { Instance::from_factory(super::GetPluginFactory()) };
    let other = unsafe { Instance::from_factory(super::GetPluginFactory()) };
    assert_eq!(instance.set(&mut [k51_4, kStereoWide], k71_2), kResultOk);
    assert_eq!(instance.set(&mut [k51], k51), kResultFalse);
    assert_eq!(instance.set(&mut [k71_2, k51], k51_4), kResultFalse);
    assert_eq!(instance.set(&mut [k71_2], kStereo), kResultFalse);
    assert_eq!(instance.get(INPUT, 0), Ok(k51_4));
    assert_eq!(instance.get(INPUT, 1), Ok(kStereoWide));
    assert_eq!(instance.get(OUTPUT, 0), Ok(k71_2));
    assert_eq!(instance.get(INPUT, -1), Err(kInvalidArgument));
    assert_eq!(other.get(INPUT, 0), Ok(k71_4));
    instance.activate();
    assert_eq!(
        OBSERVED.take(),
        Some(vec![0, 1, 2, 3, 4, 5, 12, 14, 15, 17])
    );
}

#[cfg(windows)]
#[test]
#[ignore = "requires the built surround_probe.vst3 in SURROUND_PROBE_VST3"]
fn load_vst3_factory() {
    use windows::Win32::{
        Foundation::FreeLibrary,
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    };
    use windows::core::{HSTRING, s};

    let path = std::env::var_os("SURROUND_PROBE_VST3").unwrap();
    unsafe {
        let module = LoadLibraryW(&HSTRING::from(path.as_os_str())).unwrap();
        let symbol = GetProcAddress(module, s!("GetPluginFactory")).unwrap();
        let factory: extern "system" fn() -> *mut c_void = std::mem::transmute(symbol);
        {
            let instance = Instance::from_factory(factory());
            assert_eq!(instance.set(&mut [k71_4], k71_4), kResultOk);
            assert_eq!(instance.get(INPUT, 0), Ok(k71_4));
            assert_eq!(instance.get(OUTPUT, 0), Ok(k71_4));
            instance.activate();
        }
        FreeLibrary(module).unwrap();
    }
}
