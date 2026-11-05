#!/usr/bin/env python3
"""Extract only a bounded distribution tree; never permit archive writes into CI source/credentials."""
from pathlib import Path, PurePosixPath
import sys
import tarfile

ROOT=Path(__file__).resolve().parents[1]

def unpack(target, archive=None, output=None):
    expected="Animesh.app" if target.endswith("apple-darwin") else "animesh"
    archive=archive or ROOT/"target/unsigned/bundle.tar.gz"
    output=output or ROOT/"target/bundle"
    output.mkdir(parents=True,exist_ok=True)
    with tarfile.open(archive,"r:gz") as tar:
        members=[];total=0
        for member in tar:
            members.append(member);total+=member.size
            if len(members)>5000 or total>512*1024*1024:
                raise ValueError("Distribution archive exceeds its extraction budget")
            path=PurePosixPath(member.name)
            if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0]!=expected:
                raise ValueError("Distribution archive contains an unexpected path")
        tar.extractall(output,members=members,filter="data")

if __name__=="__main__":unpack(sys.argv[1])
