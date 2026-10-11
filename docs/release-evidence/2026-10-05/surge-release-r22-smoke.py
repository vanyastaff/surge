import os, pathlib, subprocess, tempfile, tarfile, sys, json, time
archive=pathlib.Path(sys.argv[1])
base=pathlib.Path(tempfile.mkdtemp(prefix="surge-r22-",dir="/tmp"))
extract=base/"archive"
extract.mkdir()
with tarfile.open(archive) as t: t.extractall(extract)
cli=next(p for p in extract.rglob("surge") if p.is_file())
repo=base/"project"; repo.mkdir()
home=base/"home"; home.mkdir()
env=os.environ.copy(); env["GIT_CONFIG_NOSYSTEM"]="1"; env["GIT_CONFIG_GLOBAL"]="/dev/null"; env["SURGE_HOME"]=str(home); env["PATH"]=str(cli.parent)+":/usr/bin:/bin:/usr/sbin:/sbin"
log=[]
def run(args, allowed=(0,)):
    p=subprocess.run(args,cwd=repo,env=env,text=True,capture_output=True,timeout=90)
    log.append("$ "+" ".join(map(str,args))+"\nexit="+str(p.returncode)+"\n"+p.stdout+p.stderr)
    (base/"commands.log").write_text("\n".join(log))
    assert p.returncode in allowed, log[-1]
    return p
def wait_stopped():
    deadline=time.monotonic()+45
    paths=[home/"daemon"/"daemon.pid",home/"daemon"/"daemon.sock"]
    while any(path.exists() for path in paths):
        if time.monotonic()>=deadline:
            raise AssertionError("daemon stop did not settle within 45s: "+str([str(p) for p in paths if p.exists()]))
        time.sleep(0.1)
    log.append("daemon settlement: PASS; isolated PID file and socket absent")
    (base/"commands.log").write_text("\n".join(log))
try:
    run(["git","init"])
    (repo/".gitignore").write_text("surge.toml\n.surge/\n")
    (repo/"README.md").write_text("# Isolated Surge release E2E fixture\n")
    (repo/"flow.toml").write_text(pathlib.Path("/Users/vanyastafford/Develop/surge/examples/flow_terminal_only.toml").read_text())
    run(["git","add","README.md","flow.toml",".gitignore"])
    run(["git","-c","user.name=Release smoke","-c","user.email=release-smoke@example.invalid","commit","-m","Fixture"])
    run([str(cli),"--version"])
    run([str(cli.parent/"surge-daemon"),"--version"])
    run([str(cli),"init","--default"])
    run([str(cli),"project","describe","--author-mode","deterministic"])
    assert (repo/"project.md").is_file()
    run(["git","add","project.md"])
    run(["git","-c","user.name=Release smoke","-c","user.email=release-smoke@example.invalid","commit","-m","Capture generated project context"])
    assert not run(["git","status","--porcelain=v1"]).stdout
    source_head=run(["git","rev-parse","HEAD"]).stdout
    first=run([str(cli),"engine","run","flow.toml","--watch"])
    run([str(cli),"daemon","status"])
    assert not run(["git","status","--porcelain=v1"]).stdout
    second=run([str(cli),"engine","run","flow.toml","--daemon","--watch"])
    listing=run([str(cli),"engine","ls"])
    ids=[]
    import re
    for p in [first,second]:
        ids.append(next(x for x in p.stdout.splitlines() if re.fullmatch(r"(?:run-)?[0-9A-HJKMNP-TV-Z]{26}",x)))
    for rid in ids:
        replay=run([str(cli),"engine","replay",rid,"--format","json"])
        assert "completed" in replay.stdout.lower(), replay.stdout
    run([str(cli),"daemon","stop"])
    wait_stopped()
    run([str(cli),"daemon","start","--detached"])
    run([str(cli),"daemon","status"])
    run([str(cli),"engine","ls"])
    for rid in ids:
        replay=run([str(cli),"engine","replay",rid,"--format","json"])
        assert "completed" in replay.stdout.lower(), replay.stdout
    assert not run(["git","status","--porcelain=v1"]).stdout
    assert run(["git","rev-parse","HEAD"]).stdout==source_head
    run([str(cli),"daemon","stop"])
    wait_stopped()
    result="PASS"
except Exception as e:
    result="FAIL: "+str(e)
finally:
    try:
        if (home/"daemon"/"daemon.pid").exists():
            run([str(cli),"daemon","stop"],allowed=(0,1))
        wait_stopped()
    except Exception as e: log.append("cleanup: "+str(e))
    (base/"commands.log").write_text("\n".join(log))
    report=f"# R22 End-to-end release smoke\n\n{result}\n\nArchive: {archive}\nExtracted CLI: {cli}\nTemporary project: {repo}\nIsolated SURGE_HOME: {home}\nExact command outputs: {base/'commands.log'}\n\nNo real accounts or user runtime data used. No source edits or cargo builds. Terminal-only flows validate durable owner submission, daemon auto-start, storage completion, replay, and restart where commands reached. Agent/approval/external-service scenarios are unverified by this smoke.\n"
    pathlib.Path("/tmp/surge-release-r22.md").write_text(report)
    print(report)
    sys.exit(0 if result=="PASS" else 1)
