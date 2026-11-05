#!/usr/bin/python3
"""Private Linux Orca/AT-SPI observation. Never sends input or accessibility actions.

Run outside a restricted process/socket sandbox, after building crabcraft. All X11,
D-Bus, Orca, speech-dispatcher and IDE state belongs to a new private /tmp directory.
Only the installed reader and libraries are used; no external assets are distributed.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import select
import signal
import subprocess
import sys
import tempfile
import time


def private_environment(directory):
    env = {"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "LC_ALL": "C.UTF-8"}
    for key, name in [("XDG_CONFIG_HOME", "config"),
                      ("XDG_CACHE_HOME", "cache"), ("XDG_DATA_HOME", "data"),
                      ("XDG_RUNTIME_DIR", "runtime"), ("TMPDIR", "tmp"),
                      ("CRABCRAFT_HOME", "ide")]:
        path = directory / name
        path.mkdir(mode=0o700, exist_ok=True)
        env[key] = str(path)
    (directory / "fontconfig.conf").write_text('<fontconfig><dir>/usr/share/fonts</dir><cachedir>' + str(directory / "cache/fontconfig") + '</cachedir></fontconfig>')
    env.update({"NO_AT_BRIDGE": "0", "GTK_MODULES": "atk-bridge",
                "WINIT_UNIX_BACKEND": "x11", "LIBGL_ALWAYS_SOFTWARE": "1",
                "PULSE_SERVER": "unix:" + str(directory / "no-audio"),
                "SPEECHD_ADDRESS": "unix_socket:" + str(directory / "speech.sock"),
                "ALSA_CONFIG_PATH": str(directory / "alsa.conf"),
                "FONTCONFIG_FILE": str(directory / "fontconfig.conf"),
                "GTK2_RC_FILES": "/dev/null", "GTK_THEME": "Adwaita"})
    return env


def stop(process, timeout=3):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def observe(directory, duration):
    import pyatspi
    from gi.repository import GLib
    events = []
    snapshots = []
    def event_received(event):
        if len(events) >= 2000:
            return
        try:
            application = event.source.getApplication()
            app_name = application.name if application else ""
            if "crabcraft" not in app_name.lower():
                return
            events.append({"type": event.type, "name": event.source.name,
                           "detail1": event.detail1, "detail2": event.detail2})
        except Exception:
            pass
    pyatspi.Registry.registerEventListener(event_received, "object:state-changed:focused",
        "object:text-caret-moved", "object:text-changed", "window:activate")
    started = time.monotonic()
    def sample():
        nodes = []
        def walk(node, depth, tree_depth=0):
            if depth > 25 or len(nodes) >= 5000:
                return
            try:
                role = node.getRoleName()
                tree_depth += int(role == "tree item")
                record = {"name": node.name, "role": role, "attributes": node.getAttributes(), "tree_depth": tree_depth}
                try:
                    text = node.queryText()
                    record.update({"text": text.getText(0, min(text.characterCount, 16384)),
                                   "caret": text.caretOffset, "characters": text.characterCount})
                    if 0 <= text.caretOffset < text.characterCount:
                        bounds = text.getCharacterExtents(text.caretOffset, pyatspi.DESKTOP_COORDS)
                        record["caret_bounds"] = list(bounds)
                except Exception:
                    pass
                nodes.append(record)
                for child in node:
                    walk(child, depth + 1, tree_depth)
            except Exception:
                pass
        desktop = pyatspi.Registry.getDesktop(0)
        for application in desktop:
            if "crabcraft" in application.name.lower():
                walk(application, 0)
        snapshots.append(nodes)
        (directory / "atspi.json").write_text(json.dumps({"events": events, "snapshots": snapshots}, ensure_ascii=False, indent=2))
        if time.monotonic() - started >= duration:
            pyatspi.Registry.stop()
            return False
        return True
    GLib.timeout_add_seconds(1, sample)
    pyatspi.Registry.start()


def inside(directory, binary, duration):
    processes = []
    logs = []
    def launch(name, argv):
        log = (directory / (name + ".log")).open("wb")
        logs.append(log)
        process = subprocess.Popen(argv, stdout=log, stderr=subprocess.STDOUT)
        processes.append(process)
        return process
    try:
        for schema, key in [("org.gnome.desktop.interface", "toolkit-accessibility"),
                            ("org.gnome.desktop.a11y.applications", "screen-reader-enabled")]:
            subprocess.run(["gsettings", "set", schema, key, "true"], check=True)
        launch("window-manager", ["metacity", "--sm-disable", "--no-composite"])
        (directory / "alsa.conf").write_text("pcm.!default { type null }\npcm.null { type null }\n")
        config = directory / "speech-config"
        config.mkdir()
        (config / "speechd.conf").write_text('LogLevel 3\nLogDir "' + str(directory) + '"\nAddModule "dummy" "sd_dummy" ""\nDefaultModule "dummy"\nAudioOutputMethod "alsa"\nAudioALSADevice "null"\n')
        launch("speech-dispatcher", ["speech-dispatcher", "--run-single", "--config-dir", str(config), "--socket-path", str(directory / "speech.sock"), "--pid-file", str(directory / "speech.pid")])
        # Deliberately no --replace/--setup: those options terminate readers by UID.
        reader = launch("orca-launcher", ["orca", "--disable", "braille", "--user-prefs", str(directory / "orca"), "--debug", "--debug-file", str(directory / "orca-debug.log")])
        time.sleep(3)
        if reader.poll() is not None:
            raise RuntimeError("Private Orca exited; inspect orca-launcher.log. Existing readers are never replaced.")
        root = directory / "RustReader"
        (root / "src").mkdir(parents=True)
        (root / "Cargo.toml").write_text('[package]\nname="screen_reader_fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n')
        (root / "src/main.rs").write_text('fn main() {\n    let accessible_value = 42;\n    println!("Rust reader evidence: {}", accessible_value);\n}\n')
        (root / "build.rs").write_text('fn main(){std::fs::write("project-code-ran","unexpected").unwrap();}\n')
        ide = launch("ide", [str(binary), str(root), "--line", "2", "--column", "9", str(root / "src/main.rs")])
        observe(directory, duration)
        if (root / "project-code-ran").exists():
            raise RuntimeError("Opening the untrusted project executed its code")
        # Orca buffers its own debug stream; cleanly stop only this reader before inspecting it.
        stop(ide)
        time.sleep(0.5)
        stop(reader, timeout=10)
        tree = json.loads((directory / "atspi.json").read_text())
        editors = [node for snapshot in tree["snapshots"] for node in snapshot
                   if node.get("name", "").startswith("Source code:") and "accessible_value" in node.get("text", "")]
        speech = (directory / "orca-debug.log").read_text(errors="replace")
        utterances = [line for line in speech.splitlines() if "SPEECH OUTPUT:" in line]
        (directory / "speech-output.log").write_text("\n".join(utterances) + "\n")
        if not editors:
            raise RuntimeError("No source editor with real Rust text was exposed through AT-SPI")
        if not any(len(node.get("caret_bounds", [])) == 4 and node["caret_bounds"][2] > 0 and node["caret_bounds"][3] > 0 for node in editors):
            raise RuntimeError("No positive source character geometry was exposed through AT-SPI")
        if not any(node.get("role") == "tree" and node.get("name") == "Project files" for snapshot in tree["snapshots"] for node in snapshot):
            raise RuntimeError("Project files were not exposed as an AT-SPI tree")
        if not any(node.get("role") == "tree item" and node.get("name") == "src" and node.get("tree_depth", 0) >= 2 for snapshot in tree["snapshots"] for node in snapshot):
            raise RuntimeError("Explorer items do not have real nested AT-SPI parents")
        if not any("accessible_value" in line or "accessible value" in line for line in utterances):
            raise RuntimeError("Orca did not generate a source-related utterance; inspect reader logs")
        print(json.dumps({"directory": str(directory), "editor_snapshots": len(editors), "reader_utterances": len(utterances), "project_code_executed": False}))
    finally:
        for process in reversed(processes):
            stop(process)
        for log in logs:
            log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--x-tools", type=Path, default=Path("/tmp/crabcraft-accessibility/root/usr/bin"))
    parser.add_argument("--duration", type=int, default=20)
    parser.add_argument("--inside", type=Path)
    args = parser.parse_args()
    if not 5 <= args.duration <= 60:
        parser.error("duration must be between 5 and 60 seconds")
    binary = args.binary.resolve(strict=True)
    if args.inside:
        inside(args.inside, binary, args.duration)
        return
    directory = Path(tempfile.mkdtemp(prefix="crabcraft-orca-"))
    directory.chmod(0o700)
    env = private_environment(directory)
    auth = directory / "Xauthority"
    cookie = secrets.token_hex(16)
    subprocess.run([str(args.x_tools / "xauth"), "-f", str(auth), "add", ":0", "MIT-MAGIC-COOKIE-1", cookie], env=env, check=True, capture_output=True)
    read_fd, write_fd = os.pipe()
    with (directory / "xvfb.log").open("wb") as log:
        server = subprocess.Popen([str(args.x_tools / "Xvfb"), "-displayfd", str(write_fd), "-screen", "0", "1600x1000x24", "-nolisten", "tcp", "-auth", str(auth)], env=env, pass_fds=(write_fd,), stdout=log, stderr=subprocess.STDOUT)
        os.close(write_fd)
        bus = None
        try:
            if not select.select([read_fd], [], [], 10)[0]:
                raise RuntimeError("Xvfb did not assign a private display: " + str(directory))
            display = os.read(read_fd, 32).decode().strip()
            if not display.isdecimal():
                raise RuntimeError("Invalid Xvfb display: " + repr(display))
            subprocess.run([str(args.x_tools / "xauth"), "-f", str(auth), "add", ":" + display, "MIT-MAGIC-COOKIE-1", cookie], env=env, check=True, capture_output=True)
            env.update({"DISPLAY": ":" + display, "XAUTHORITY": str(auth)})
            print("Private reader evidence: " + str(directory), flush=True)
            bus = subprocess.Popen(["dbus-run-session", "--", "/usr/bin/python3", str(Path(__file__).resolve()), "--binary", str(binary), "--duration", str(args.duration), "--inside", str(directory)], env=env, start_new_session=True)
            result = bus.wait(timeout=args.duration + 30)
            if result:
                raise SystemExit(result)
        finally:
            os.close(read_fd)
            if bus is not None and bus.poll() is None:
                os.killpg(bus.pid, signal.SIGTERM)
                bus.wait(timeout=5)
            stop(server)


if __name__ == "__main__":
    main()
