import subprocess,pathlib,json,time,shutil
checks=[
('clippy-all',['cargo','clippy','--locked','--workspace','--all-targets','--all-features','--','-D','warnings']),
('clippy-default',['cargo','clippy','--locked','--workspace','--all-targets','--','-D','warnings']),
('fmt',['cargo','fmt','--all','--check']),
('msrv',['cargo','+1.96.0','check','--locked','--workspace','--exclude','surge-ui','--all-targets']),
('nextest',['cargo','nextest','run','--locked','--workspace','--no-fail-fast','--test-threads','4']),
]
results=[]
for name,cmd in checks:
 log=pathlib.Path('/tmp/surge-release-frozen-'+name+'.log'); print('START',name,flush=True); started=time.time()
 with log.open('w') as f: result=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT)
 record={'name':name,'command':cmd,'exit':result.returncode,'seconds':round(time.time()-started,2),'log':str(log)}; results.append(record); pathlib.Path('/tmp/surge-release-frozen-results.json').write_text(json.dumps(results,indent=2)); print('END',name,'exit',result.returncode,flush=True)
 cache=pathlib.Path('target/debug/incremental')
 if cache.exists(): shutil.rmtree(cache); cache.mkdir()
 if result.returncode and name!='nextest': break
