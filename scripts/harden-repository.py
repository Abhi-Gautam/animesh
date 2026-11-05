#!/usr/bin/env python3
"""Apply the audited repository controls. No releases, branch pushes, or messages are sent."""
import argparse
import json
import subprocess

REPO = "Abhi-Gautam/animesh"

def api(path, method="GET", body=None):
    command=["gh","api",path,"--method",method]
    if body is not None:command += ["--input","-"]
    result=subprocess.run(command,input=json.dumps(body) if body is not None else None,text=True,capture_output=True)
    if result.returncode:raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout) if result.stdout.strip() else None

def apply():
    root=f"repos/{REPO}"
    before=api(root)
    assert before["permissions"]["admin"], "Repository administrator access required"
    owner=api("users/Abhi-Gautam")
    api(root+"/actions/permissions/workflow","PUT",{"default_workflow_permissions":"read","can_approve_pull_request_reviews":False})
    for endpoint in ["vulnerability-alerts","automated-security-fixes","private-vulnerability-reporting"]:
        api(root+"/"+endpoint,"PUT")
    api(root,"PATCH",{"security_and_analysis":{"secret_scanning":{"status":"enabled"},"secret_scanning_push_protection":{"status":"enabled"}}})
    api(root+"/environments/release","PUT",{
        "wait_timer":0,"reviewers":[{"type":"User","id":owner["id"]}],
        "prevent_self_review":False,"can_admins_bypass":False,
        "deployment_branch_policy":{"protected_branches":False,"custom_branch_policies":True}})
    policies=api(root+"/environments/release/deployment-branch-policies")["branch_policies"]
    if not any(p["name"]=="v*" and p["type"]=="tag" for p in policies):
        api(root+"/environments/release/deployment-branch-policies","POST",{"name":"v*","type":"tag"})
    api(root+"/branches/master/protection","PUT",{
        "required_status_checks":{"strict":True,"contexts":["check (macos-14)","check (macos-15-intel)","check (ubuntu-24.04)","check (ubuntu-24.04-arm)","linux-daemon","bundle","layering","Security gates"]},
        "enforce_admins":True,"required_pull_request_reviews":{"dismiss_stale_reviews":True,"require_code_owner_reviews":False,"required_approving_review_count":0},
        "restrictions":None,"required_conversation_resolution":True,"allow_force_pushes":False,"allow_deletions":False})
    existing={r["name"]:r["id"] for r in api(root+"/rulesets")}
    for name,rules,bypass in [
        ("Immutable release tags",[{"type":"update"},{"type":"deletion"}],[]),
        ("Release tag creation",[{"type":"creation"}],[{"actor_id":5,"actor_type":"RepositoryRole","bypass_mode":"always"}])]:
        body={"name":name,"target":"tag","enforcement":"active","bypass_actors":bypass,
              "conditions":{"ref_name":{"include":["refs/tags/v*"],"exclude":[]}},"rules":rules}
        path=root+"/rulesets"+(f"/{existing[name]}" if name in existing else "")
        api(path,"PUT" if name in existing else "POST",body)
    print("Enabled read-only workflow defaults, scanning, protected master/release tags, and a reviewed tag-only release environment.")

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--apply",action="store_true",required=True);parser.parse_args();apply()
