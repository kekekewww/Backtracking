use clap_sys::{
    ext::{audio_ports::*, audio_ports_config::*},
    factory::plugin_factory::*,
    host::clap_host,
    version::CLAP_VERSION,
};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
};

unsafe extern "C" fn no_extension(_: *const clap_host, _: *const c_char) -> *const c_void {
    ptr::null()
}
unsafe extern "C" fn no_request(_: *const clap_host) {}

#[test]
fn default_and_surround_configurations_match_their_ports() {
    let host = clap_host {
        clap_version: CLAP_VERSION,
        host_data: ptr::null_mut(),
        name: c"S3-3".as_ptr(),
        vendor: c"S3-3".as_ptr(),
        url: c"".as_ptr(),
        version: c"1".as_ptr(),
        get_extension: Some(no_extension),
        request_restart: Some(no_request),
        request_process: Some(no_request),
        request_callback: Some(no_request),
    };
    unsafe {
        let entry = &super::clap_entry;
        assert!(entry.init.unwrap()(c"Backtracking.clap".as_ptr()));
        let factory = entry.get_factory.unwrap()(CLAP_PLUGIN_FACTORY_ID.as_ptr())
            .cast::<clap_plugin_factory>();
        let plugin = (*factory).create_plugin.unwrap()(
            factory,
            &host,
            c"tw.kamelyang.backtracking".as_ptr(),
        );
        assert!(!plugin.is_null());
        assert!((*plugin).init.unwrap()(plugin));
        let extension = (*plugin).get_extension.unwrap();
        let configs = &*extension(plugin, CLAP_EXT_AUDIO_PORTS_CONFIG.as_ptr())
            .cast::<clap_plugin_audio_ports_config>();
        let ports =
            &*extension(plugin, CLAP_EXT_AUDIO_PORTS.as_ptr()).cast::<clap_plugin_audio_ports>();
        let read = |input| {
            let mut info = std::mem::zeroed();
            assert!(ports.get.unwrap()(plugin, 0, input, &mut info));
            info
        };
        assert_eq!(read(true).channel_count, 2);
        assert_eq!(CStr::from_ptr(read(true).port_type), CLAP_PORT_STEREO);
        assert_eq!(configs.count.unwrap()(plugin), 24);
        for id in 0..24 {
            let mut config = std::mem::zeroed();
            assert!(configs.get.unwrap()(plugin, id, &mut config));
            assert_eq!(config.id, id);
            assert!(config.has_main_input && config.has_main_output);
            assert!(configs.select.unwrap()(plugin, id));
            let width = if id < 6 {
                [2, 2, 1, 1, 2, 1][id as usize]
            } else {
                6 + 2 * ((id - 6) / 3)
            };
            assert_eq!(
                (
                    config.main_input_channel_count,
                    config.main_output_channel_count
                ),
                (width, width)
            );
            assert_eq!(
                (read(true).channel_count, read(false).channel_count),
                (width, width)
            );
            if width > 2 {
                assert!(
                    config.main_input_port_type.is_null() && config.main_output_port_type.is_null()
                );
                assert!(read(true).port_type.is_null() && read(false).port_type.is_null());
                assert_eq!(config.input_port_count, 1 + u32::from(id % 3 != 0));
                assert_eq!(config.output_port_count, 1);
                if id % 3 != 0 {
                    let mut side = std::mem::zeroed();
                    assert!(ports.get.unwrap()(plugin, 1, true, &mut side));
                    assert_eq!(side.channel_count, id % 3);
                }
            }
        }
        (*plugin).destroy.unwrap()(plugin);
        entry.deinit.unwrap()();
    }
}
