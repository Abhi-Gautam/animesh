#!/usr/bin/env python3
"""Exercise the installed Linux webview against the real local daemon."""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import time
import urllib.error
import urllib.request

URL = "http://127.0.0.1:4444"
ELEMENT = "element-6066-11e4-a52e-4f735466cecf"


def request(method, path, data=None):
    payload = json.dumps(data).encode() if data is not None else None
    req = urllib.request.Request(URL + path, payload, {"Content-Type": "application/json"}, method=method)
    with urllib.request.urlopen(req, timeout=40) as response:
        result = json.load(response)["value"]
    if isinstance(result, dict) and "error" in result:
        raise RuntimeError(result)
    return result


def wait(check, label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise RuntimeError(f"Timed out: {label}")


def smoke(binary, screenshots):
    session = None
    with open(screenshots / "driver.log", "w") as log:
        # Connect directly to the same native driver Tauri uses. Its proxy can
        # reuse a closed upstream connection and lose a navigation command.
        env = dict(os.environ, TAURI_WEBVIEW_AUTOMATION="true")
        driver = subprocess.Popen(["WebKitWebDriver", "--port=4444", "--host=127.0.0.1"], env=env, stdout=log, stderr=log)
        try:
            wait(lambda: request("GET", "/status"), "WebDriver startup")
            session = request("POST", "/session", {"capabilities": {"alwaysMatch": {
                "webkitgtk:browserOptions": {"binary": str(binary.resolve())}}}})["sessionId"]
            base = f"/session/{session}"

            def execute(script, args=None):
                return request("POST", base + "/execute/sync", {"script": script, "args": args or []})

            def click(selector):
                element = request("POST", base + "/element", {"using": "css selector", "value": selector})
                request("POST", base + f"/element/{element[ELEMENT]}/click", {})

            wait(lambda: execute("return document.querySelector('#engine-state .healthy') !== null && document.querySelector('#home .page-footer') !== null"), "real daemon connection and Home render")
            assert execute("return location.origin") != "null", "webview loaded with a null origin"
            for theme in ["dark", "light"]:
                click(f'#theme option[value="{theme}"]')
                wait(lambda: execute("return document.documentElement.dataset.theme === arguments[0]", [theme]), f"{theme} theme")
                for width, height in [(1440, 900), (900, 650)]:
                    request("POST", base + "/window/rect", {"width": width, "height": height})
                    for screen in ["home", "discover", "search", "schedule", "library", "health"]:
                        click(f'nav a[href="#{screen}"]')
                        wait(lambda: execute("return location.hash === '#' + arguments[0] && !document.getElementById(arguments[0]).hidden && document.querySelector('#' + arguments[0] + ' .page-footer') !== null && !document.querySelector('#' + arguments[0] + ' .loading')", [screen]), f"{screen} render")
                        request("POST", base + "/execute/async", {
                            "script": "const done = arguments[arguments.length - 1]; requestAnimationFrame(() => requestAnimationFrame(() => done()));", "args": []})
                        geometry = execute("""
                            const page = document.getElementById(arguments[0]);
                            const footer = page.querySelector('.page-footer').getBoundingClientRect();
                            const body = page.querySelector('.page-body').getBoundingClientRect();
                            return {footerTop: footer.top, footerBottom: footer.bottom, bodyBottom: body.bottom,
                                height: innerHeight, pageHeight: document.documentElement.scrollHeight};
                        """, [screen])
                        png = request("GET", base + "/screenshot")
                        (screenshots / f"{screen}-{width}-{theme}.png").write_bytes(base64.b64decode(png))
                        assert geometry["footerBottom"] <= geometry["height"] + 1, (screen, geometry)
                        assert geometry["bodyBottom"] <= geometry["footerTop"] + 1, (screen, geometry)
                        assert geometry["pageHeight"] <= geometry["height"] + 1, (screen, geometry)
                        print(f"{screen} {width}x{height} {theme}: connected, footer visible, no page overflow", flush=True)
            click('nav a[href="#home"]')
            wait(lambda: execute("return document.querySelector('#home .page-footer button') !== null"), "Home action")
            click("#home .page-footer button")
            wait(lambda: execute("return location.hash === '#schedule'"), "Open schedule navigation")
            print("Installed desktop and Open schedule action passed.", flush=True)
        finally:
            try:
                if session:
                    request("DELETE", f"/session/{session}")
            finally:
                driver.terminate()
                try:
                    driver.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    driver.kill()
                    driver.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--screenshots", type=Path, default=Path("target/desktop-smoke"))
    args = parser.parse_args()
    args.screenshots.mkdir(parents=True, exist_ok=True)
    smoke(args.binary, args.screenshots)
