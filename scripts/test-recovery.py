"""End-to-end security checks using explicitly isolated test-harness binaries."""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import http.server
import threading

ROOT=Path(__file__).resolve().parents[1]
CLI=ROOT/"target/debug/animesh"
APP=ROOT/"target/debug/animesh-app"

def check():
    for binary in [CLI,APP]:
        probe=subprocess.run([str(binary),"--test-harness"],text=True,capture_output=True,timeout=5)
        assert probe.returncode==0 and probe.stdout.strip()=="animesh-isolated-harness","Refusing to test binaries without explicit harness isolation"
    # Short, private paths fit Unix socket limits on macOS as well as Linux.
    with tempfile.TemporaryDirectory(prefix="animesh-e2e-",dir="/tmp") as temp:
        root=Path(temp);data=root/"data";logs=root/"logs"
        class UnexpectedNetwork(http.server.BaseHTTPRequestHandler):
            calls=0
            def do_GET(self):
                type(self).calls+=1;self.send_response(503);self.end_headers()
            do_POST=do_GET
            def log_message(self,*args):pass
        mock=http.server.HTTPServer(("127.0.0.1",0),UnexpectedNetwork)
        threading.Thread(target=mock.serve_forever,daemon=True).start()
        source_url=f"http://127.0.0.1:{mock.server_port}"
        env={**os.environ,"ANIMESH_SOURCE_URL":source_url,"ANIMESH_TVMAZE_URL":source_url,"ANIMESH_DATA_ROOT":str(data),"ANIMESH_LOG_ROOT":str(logs),"ANIMESH_HEADLESS":"1"}
        daemon=None
        def cli(*args):
            result=subprocess.run([str(CLI),"--json",*args],env=env,text=True,capture_output=True,timeout=40)
            assert result.returncode==0,result.stderr+result.stdout[:1000]
            value=json.loads(result.stdout);assert value["ok"],value;return value["data"]
        def start():
            nonlocal daemon
            daemon=subprocess.Popen([str(APP)],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            for _ in range(100):
                result=subprocess.run([str(CLI),"--json","status"],env=env,text=True,capture_output=True,timeout=5)
                if result.returncode==0 and json.loads(result.stdout)["data"]["database_ready"]:return
                assert daemon.poll() is None,"Isolated daemon exited before becoming ready"
                time.sleep(.05)
            raise AssertionError("Isolated daemon did not become ready")
        def stop():
            nonlocal daemon
            if daemon is not None:
                daemon.terminate();daemon.wait(timeout=10);daemon=None
        try:
            start();assert cli("list")==[];stop()
            conn=sqlite3.connect(data/"library.db")
            for i in range(1,502):
                conn.execute("INSERT INTO media(media_id,kind,display_title,created_at,updated_at) VALUES (?,'anime',?,1,1)",(i,"🚀"*512))
                conn.execute("INSERT INTO source_media(source_media_id,source,source_id,media_id,created_at,updated_at) VALUES (?,'anilist',?,?,1,1)",(i,i,i))
                conn.execute("INSERT INTO follows VALUES (?,'active',1,1)",(i,))
                conn.execute("INSERT INTO source_refresh_state(source_media_id,refresh_after,updated_at) VALUES (?,?,1)",(i,int(time.time())+365*86400))
            conn.commit();conn.close()
            start();rows=cli("list");assert len(rows)==501;assert len({row["media_id"] for row in rows})==501;stop()
            cli("library","backup")
            conn=sqlite3.connect(data/"library.db");conn.execute("UPDATE media SET display_title='changed' WHERE media_id=1");conn.commit();conn.close()
            cli("library","restore","--from","manual","--yes")
            conn=sqlite3.connect(data/"library.db");assert conn.execute("SELECT display_title FROM media WHERE media_id=1").fetchone()[0]=="🚀"*512;conn.close()
            cli("privacy","forget","media:1","--yes")
            assert not list((data/"backups").glob("*.db")),"Privacy deletion retained an app backup"
            assert not list((data/"recovery-original").iterdir()),"Privacy deletion retained recovery bytes"
            cli("privacy","notifications","private")
            conn=sqlite3.connect(data/"library.db");assert conn.execute("SELECT private_notifications FROM app_preferences").fetchone()[0]==1;conn.close()
            cli("library","storage")
            start();assert len(cli("list"))==500
            assert UnexpectedNetwork.calls==0,"Read/recovery checks unexpectedly triggered source refresh"
            print("Passed isolated CLI pagination (501 maximum-length titles), backup/restore, privacy erasure, and restart checks")
        finally:
            stop();mock.shutdown();mock.server_close()

if __name__=="__main__":check()
