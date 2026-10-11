import subprocess,pathlib,json,time,os,shutil
checks=[
('clippy-all',['cargo','clippy','--locked','--workspace','--all-targets','--all-features','--','-D','warnings']),
('clippy-default',['cargo','clippy','--locked','--workspace','--all-targets','--','-D','warnings']),
('fmt',['cargo','fmt','--all','--check']),
('doc',['cargo','test','--locked','--workspace','--doc']),
('msrv',['cargo','+1.96.0','check','--locked','--workspace','--exclude','surge-ui','--all-targets']),
('build-acp-mock',['cargo','build','--locked','-p','surge-acp','--bin','mock_acp_agent']),
('build-owner',['cargo','build','--locked','-p','surge-cli','--bin','surge','-p','surge-daemon','--bin','surge-daemon']),
('build-mcp-mock',['cargo','build','--locked','-p','surge-mcp','--example','mock_mcp_server','--features','mock-server']),
('ignored-acp',['cargo','test','--locked','-p','surge-acp','--test','bridge_rate_limit_classification','--test','reconnect_integration_test','--','--ignored']),
('ignored-engine',['cargo','test','--locked','-p','surge-orchestrator','--test','engine_e2e_linear_pipeline','--test','engine_concurrent_runs','--test','engine_resume_after_crash','--','--ignored']),
('ignored-mcp',['cargo','test','--locked','-p','surge-mcp','--features','mock-server','--test','mcp_stdio_e2e','--','--ignored']),
('ignored-daemon',['cargo','test','--locked','-p','surge-daemon','--test','live_provider_smoke','controlled_daemon_mcp_smoke','--','--ignored','--exact']),
('ignored-restart',['cargo','test','--locked','-p','surge-cli','--test','daemon_restart','--','--ignored']),
]
results=[]
for name,cmd in checks:
 log=pathlib.Path('/tmp/surge-release-integrated-'+name+'.log'); print('START',name,cmd,flush=True); started=time.time(); env=os.environ.copy()
 if name=='msrv': env['CARGO_INCREMENTAL']='0'
 with log.open('w') as f: result=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT,env=env)
 record={'name':name,'command':cmd,'exit':result.returncode,'seconds':round(time.time()-started,2),'log':str(log)}; results.append(record); pathlib.Path('/tmp/surge-release-integrated-results.json').write_text(json.dumps(results,indent=2)); print('END',name,'exit',result.returncode,flush=True)
 cache=pathlib.Path('target/debug/incremental')
 if cache.exists(): shutil.rmtree(cache); cache.mkdir(); print('CACHE: completed incremental artifacts cleared',flush=True)
 if result.returncode and name.startswith(('clippy','fmt','build')): break
