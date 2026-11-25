"""macOS C ABI integration checks. Run from the repo root after building examples,
the host debug bins and (optionally) the release Agent package. Only writes under target.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import platform
import select
import stat
import subprocess
import tempfile
import time
import zipfile

ROOT = Path(__file__).resolve().parents[3]
BIN = ROOT / "target/debug"
SDK = ROOT / "crates/backtracking-addon"
PLATFORM = {"arm64": "macos-aarch64", "x86_64": "macos-x86_64"}[platform.machine()]


def eventually(check, timeout=5):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        value = check()
        if value:
            return value
        time.sleep(.02)
    raise AssertionError(f"Timed out waiting for {check}")


def send(proc, obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def receive(proc):
    if not select.select([proc.stdout], [], [], 15)[0]:
        raise AssertionError("Timed out waiting for JSON response")
    return json.loads(proc.stdout.readline())


@contextlib.contextmanager
def process(args, env):
    proc = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            text=True, env=env)
    try:
        yield proc
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        proc.stdout.close()


class Host:
    def __init__(self, proc):
        self.proc = proc

    def call(self, op, **args):
        send(self.proc, dict(op=op, **args))
        reply = receive(self.proc)
        if op == "snapshot":
            return reply
        assert reply["ok"], reply
        return reply["value"]

    def install(self, path):
        summary = self.call("inspect", path=str(path))
        self.call("install", path=str(path), review_id=summary["review_id"])
        return summary["manifest"]["id"]

    def addon(self, addon_id):
        return next(a for a in self.call("snapshot")["addons"] if a["id"] == addon_id)

    def running(self, addon_id):
        eventually(lambda: self.addon(addon_id)["state"] == "running")


def fixture(directory, example, program, config=None):
    manifest = json.loads((SDK / f"examples/{example}-package/addon.json").read_text())
    manifest["run"] = {PLATFORM: {"program": "bin/example", "args": []}}
    if config:
        manifest.update(config)
    path = directory / (manifest["id"] + ".btaddon")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("addon.json", json.dumps(manifest))
        archive.write(program, "bin/example")
    return path


def check_mode(path, expected):
    assert stat.S_IMODE(path.stat().st_mode) == expected, (path, oct(path.stat().st_mode))


def lifecycle(host, root, work, env):
    tone = fixture(work, "test-tone", SDK / "target/debug/examples/test_tone")
    addon = host.install(tone)
    host.call("set_enabled", id=addon, enabled=True)
    host.running(addon)
    src = eventually(lambda: host.addon(addon)["sources"])[0]
    subprocess.run([BIN / "bt-check-audio", src["sender"]], env=env, check=True)
    host.call("select_format", id=addon, source=src["id"], format="stereo")
    installed = root / "addons" / addon / "bin/example"
    subprocess.run(["codesign", "--verify", "--strict", installed], check=True)
    attributes = subprocess.check_output(["xattr", installed], text=True).splitlines()
    print(f"Local executable mode {oct(installed.stat().st_mode & 0o777)}, quarantine={'com.apple.quarantine' in attributes}")
    for folder in [root, root/"addons", installed.parent, installed.parent.parent,
                   root/"addon-data", root/"addon-data"/addon]:
        check_mode(folder, 0o700)
    for name in ["addon-host-v1.json", "addon-host-endpoint-v1.json", ".addon-host.lock"]:
        check_mode(root/name, 0o600)
    host.call("stop", id=addon)
    eventually(lambda: host.addon(addon)["state"] == "stopped")
    host.call("remove", id=addon, delete_data=True)
    control = host.install(fixture(work, "control", SDK / "target/debug/examples/control_example"))
    host.call("start", id=control)
    host.running(control)
    eventually(lambda: host.addon(control)["control_asked"])
    assert not host.addon(control)["control_allowed"]
    host.call("set_control_allowed", id=control, allowed=True)
    call = eventually(lambda: host.call("take_control"))[0]
    assert call["command"]["command"] == "get_state"
    host.call("control_reply", addon_id=control, run=call["run"], request_id=call["request_id"],
              error={"code": "no_audio", "message": "Test host has no capture"})
    host.call("set_control_allowed", id=control, allowed=False)
    assert not host.addon(control)["control_allowed"]
    host.call("set_control_allowed", id=control, allowed=True)
    host.install(work / (control + ".btaddon"))
    assert host.addon(control)["control_allowed"]
    host.call("stop", id=control)
    eventually(lambda: host.addon(control)["state"] == "stopped")
    host.call("remove", id=control, delete_data=True)
    assert host.call("take_control")==[], "Removed run left an executable control request"
    print("Tone audio, file modes, control request/reply/revoke and removal passed")


def agent_flow(host, root, package, env):
    addon = host.install(package)
    host.call("set_enabled", id=addon, enabled=True)
    host.running(addon)
    data = root / "addon-data" / addon
    eventually(lambda: (data/"endpoint.json").is_file())
    for name in ["endpoint.json", "token"]:
        check_mode(data/name, 0o600)
    program = root / "addons" / addon / "bin/backtracking-agent"
    with process([program, "--stdio"], env) as client:
        send(client, {"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"host-test","version":"1"}}})
        assert "result" in receive(client)
        send(client, {"jsonrpc":"2.0","method":"notifications/initialized"})
        host.call("set_control_allowed", id=addon, allowed=True)
        def query(i):
            send(client, {"jsonrpc":"2.0","id":i,"method":"tools/call",
                          "params":{"name":"get_state","arguments":{}}})
        query(2)
        call = eventually(lambda: host.call("take_control"))[0]
        assert call["addon_id"]==addon, call
        span = dict(start_frame=0, end_frame=0, seconds=0)
        state = dict(capturing=False,recording=False,sample_rate=48000,lanes=[],history=span,view=span,
                     separate_rows=False,preview_playing=False,save_folder="",drag_folder="")
        host.call("control_reply",addon_id=addon,run=call["run"],request_id=call["request_id"],result={"state":state})
        response=receive(client)
        assert not response["result"].get("isError"), response
        host.call("set_control_allowed",id=addon,allowed=False)
        query(3)
        denied=receive(client)
        assert denied["result"]["isError"] and "not_allowed" in json.dumps(denied), denied
    print("Backtracking Agent installed, running, get_state returned state, revoked get_state returned not_allowed")
    host.call("stop", id=addon)
    eventually(lambda: host.addon(addon)["state"]=="stopped")
    host.call("remove",id=addon,delete_data=True)


def dead(pid):
    try:
        os.kill(pid, 0)
        return False
    except ProcessLookupError:
        return True


def crash_checks(host, root, work):
    bad_manifest = {"id":"com.example.no-mac","run":{"windows-x86_64":{"program":"bin/example"}}}
    path = fixture(work,"control","/bin/sleep",bad_manifest)
    send(host.proc,dict(op="inspect",path=str(path)))
    rejected=receive(host.proc)
    assert not rejected["ok"] and PLATFORM in rejected["message"],rejected
    marked=fixture(work,"control","/bin/sleep",{"id":"com.example.marked"})
    # Synthetic quarantine metadata, never launch this sample.
    attr="com.apple.quarantine"
    subprocess.run(["xattr","-w",attr,"0081;00000001;BacktrackingTest;",marked],check=True)
    marked_id=host.install(marked)
    assert subprocess.check_output(["xattr","-p",attr,root/"addons"/marked_id/"bin/example"]) == subprocess.check_output(["xattr","-p",attr,marked])
    host.call("remove",id=marked_id,delete_data=True)
    quick=fixture(work,"control","/usr/bin/false",{"id":"com.example.quits"})
    addon=host.install(quick)
    host.call("start",id=addon)
    eventually(lambda: host.addon(addon)["state"]=="failed")
    assert "3 exits within 60 seconds" in host.addon(addon)["reason"]
    # A script with a subprocess exercises group cleanup; neither cooperates with the host.
    slow=work/"no-hello"
    slow.write_text('#!/bin/sh\nsleep 60 &\necho $! > "$BACKTRACKING_ADDON_DATA/child.pid"\nwait\n')
    slow.chmod(0o700)
    sleep_id=host.install(fixture(work,"control",slow,{"id":"com.example.no-hello"}))
    host.call("start",id=sleep_id)
    eventually(lambda:host.addon(sleep_id)["state"]=="not_responding",timeout=16)
    host.call("start",id=sleep_id)
    start=time.monotonic()
    host.call("stop",id=sleep_id)
    eventually(lambda:host.addon(sleep_id)["state"]=="stopped")
    elapsed=time.monotonic()-start
    assert 1.8<=elapsed<4,elapsed
    pidfile=root/"addon-data"/sleep_id/"child.pid"
    pidfile.unlink(missing_ok=True)
    host.call("start",id=sleep_id)
    pid=eventually(lambda:host.addon(sleep_id)["pid"])
    descendant=eventually(lambda: int(pidfile.read_text()) if pidfile.exists() and pidfile.stat().st_size else None)
    start=time.monotonic()
    host.proc.kill()
    host.proc.wait()
    eventually(lambda:dead(pid) and dead(descendant),timeout=3)
    print(f"10 s hello timeout, three crashes, {elapsed:.3f} s stop, SIGKILL cleanup {(time.monotonic()-start)*1000:.1f} ms")


if __name__=="__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("--agent", type=Path)
    opts=parser.parse_args()
    with tempfile.TemporaryDirectory(dir=ROOT/"target") as tmp:
        work=Path(tmp)
        root=work/"data"
        env=dict(os.environ,HOME=str(work))
        with process([BIN/"bt-addon-cli",root],env) as proc:
            host=Host(proc)
            eventually(lambda:host.call("snapshot")["ready"])
            lifecycle(host,root,work,env)
            if opts.agent:
                agent_flow(host,root,opts.agent.resolve(),env)
            crash_checks(host,root,work)
