//! Line-oriented C ABI test driver. Pass an absolute root and optional "secondary".
//! Read one JSON document per line. Commands wait for completion; {"op":"snapshot"}
//! reads the current publication. EOF releases the handle.
#[cfg(unix)]
mod unix {
    use backtracking_addon_host::*;
    use serde_json::{Value, json};
    use std::{
        ffi::{CStr, CString},
        io::{self, BufRead},
        time::{Duration, Instant},
    };
    fn decode(raw: *mut std::ffi::c_char) -> Value {
        unsafe {
            let v = serde_json::from_slice(CStr::from_ptr(raw).to_bytes())
                .expect("host returned invalid JSON");
            bt_addon_string_free(raw);
            v
        }
    }
    fn command(h: *mut BtAddonHost, v: &Value) -> Value {
        let s = CString::new(v.to_string()).unwrap();
        decode(unsafe { bt_addon_host_command(h, s.as_ptr()) })
    }
    pub(super) fn run() {
        let root =
            CString::new(std::env::args().nth(1).expect("Pass an absolute data root")).unwrap();
        let role = u32::from(std::env::args().nth(2).as_deref() == Some("secondary"));
        let host = unsafe { bt_addon_host_new(root.as_ptr(), c"0.2.1".as_ptr(), role) };
        assert!(!host.is_null(), "Invalid data root");
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            let Ok(mut value) = serde_json::from_str::<Value>(&line) else {
                println!("null");
                continue;
            };
            if !value.is_object() {
                println!("null");
                continue;
            }
            value["schema"] = 1.into();
            let mut result = if value["op"] == "snapshot" {
                decode(unsafe { bt_addon_host_snapshot(host) })
            } else {
                command(host, &value)
            };
            if let Some(ticket) = result["ticket"].as_u64() {
                let until = Instant::now() + Duration::from_secs(125);
                loop {
                    result = command(
                        host,
                        &json!({"schema":1,"op":"take_result","ticket":ticket}),
                    );
                    if result["code"] != "busy" || Instant::now() >= until {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(3));
                }
            }
            println!("{result}");
        }
        unsafe { bt_addon_host_free(host) }
    }
}
#[cfg(unix)]
fn main() {
    unix::run()
}
#[cfg(not(unix))]
fn main() {
    eprintln!("This add-on host requires Unix");
    std::process::exit(1)
}
