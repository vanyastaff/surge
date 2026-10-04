use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use surge_core::mcp_config::{McpServerRef, McpTransportConfig};

pub(crate) const CHILD: &str = r#"
import json, os, sys
with open(os.environ['RECORDER'], 'w') as f:
    json.dump({'exe': os.path.realpath(sys.executable), 'args': sys.argv,
               'env': {key: os.environ[key] for key in ['PRIVATE_VALUE', 'RECORDER', 'PAGINATE', 'STDERR_STRESS'] if key in os.environ},
               'cwd': os.getcwd(), 'pid': os.getpid()}, f)
with open(os.environ['RECORDER'] + '.spawns', 'a') as f:
    f.write(str(os.getpid()) + '\n')
private = sys.argv[2:] + [os.environ['PRIVATE_VALUE']]
for item in private:
    os.write(2, item.encode('utf-8') + b'\r\n')
if os.environ.get('STDERR_STRESS'):
    os.write(2, b'\xff\xfe\r\n')
    os.write(2, b'H' * (2 * 1024 * 1024) + b'\r\n')
    for _ in range(600):
        os.write(2, b'arbitrary-private-no-heuristic\n')
for line in sys.stdin:
    msg = json.loads(line)
    if 'id' not in msg:
        continue
    method = msg['method']
    with open(os.environ['RECORDER'] + '.requests', 'a') as f:
        f.write(method + '\n')
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': 'oracle', 'version': '1'}, 'instructions': private[-1]}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'echo', 'description': 'legitimate tool', 'inputSchema': {'type': 'object'}}]}
        if os.environ.get('PAGINATE'):
            if msg.get('params', {}).get('cursor') == 'second':
                result['tools'][0]['name'] = 'echo-second'
            else:
                result['nextCursor'] = 'second'
    elif msg['params']['name'] == 'rpc_error':
        print(json.dumps({'jsonrpc': '2.0', 'id': msg['id'], 'error': {'code': -32603, 'message': private[-1], 'data': private}}), flush=True)
        continue
    elif msg['params']['name'] == 'error':
        result = {'isError': True, 'content': [{'type': 'text', 'text': '\n'.join(private)}, {'type': 'image', 'data': private[-1], 'mimeType': 'image/png'}],
                  'structuredContent': {'private': private}, '_meta': {'private': private}}
    else:
        result = {'isError': False, 'content': [{'type': 'text', 'text': 'functional exact Ω\nline'}],
                  'structuredContent': {'value': 7}, '_meta': {'public': 'exact'}}
    print(json.dumps({'jsonrpc': '2.0', 'id': msg['id'], 'result': result}), flush=True)
if os.environ.get('STDERR_STRESS'):
    os.write(2, b'final-private-no-newline')
"#;

pub(crate) struct Fixture {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) config: McpServerRef,
    pub(crate) private: Vec<String>,
    pub(crate) args: Vec<String>,
}

impl Fixture {
    pub(crate) fn new(name: &str) -> Self {
        let dir = tempfile::tempdir().expect("private fixture directory");
        let script = dir.path().join("child.py");
        std::fs::write(&script, CHILD).expect("fixture script");
        let private: Vec<String> = [
            "xy",
            "xy-overlap",
            "line-one\nline-two",
            "私有Ω",
            "quote\"slash\\end",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let mut args = vec![
            script.to_string_lossy().into_owned(),
            "ordered-first".into(),
        ];
        args.extend(private.iter().cloned());
        let env = HashMap::from([
            (
                "RECORDER".into(),
                dir.path()
                    .join("private-recorder.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("PRIVATE_VALUE".into(), "environment-private".into()),
        ]);
        let config = McpServerRef::new(
            name.into(),
            McpTransportConfig::stdio(PathBuf::from("/usr/bin/python3"), args.clone(), env),
            None,
            Duration::from_secs(10),
            false,
        );
        Self {
            dir,
            config,
            private,
            args,
        }
    }

    pub(crate) fn verify_transport(&self) {
        let recorded: serde_json::Value = serde_json::from_slice(
            &std::fs::read(self.dir.path().join("private-recorder.json")).expect("child recorder"),
        )
        .expect("recorder JSON");
        let executable = std::process::Command::new("/usr/bin/python3")
            .args([
                "-c",
                "import os,sys; print(os.path.realpath(sys.executable))",
            ])
            .output()
            .expect("independent executable identity");
        assert!(
            executable.status.success(),
            "executable identity probe failed"
        );
        let executable = String::from_utf8(executable.stdout).expect("executable path");
        assert_eq!(
            recorded["exe"].as_str(),
            Some(executable.trim()),
            "executable altered"
        );
        assert!(
            recorded["args"] == serde_json::json!(self.args),
            "ordered arguments altered"
        );
        assert!(
            recorded["env"]["PRIVATE_VALUE"] == "environment-private",
            "declared environment altered"
        );
        let cwd = std::fs::canonicalize(self.dir.path()).expect("fixture directory identity");
        assert_eq!(recorded["cwd"].as_str(), cwd.to_str(), "cwd altered");
    }

    pub(crate) fn assert_opaque(&self, public: &str) {
        for sentinel in self
            .private
            .iter()
            .map(String::as_str)
            .chain(["environment-private"])
        {
            let escaped = serde_json::to_string(sentinel).expect("escaped sentinel");
            assert!(
                !public.contains(sentinel),
                "external diagnostic escaped into public observation"
            );
            assert!(
                !public.contains(&escaped[1..escaped.len() - 1]),
                "escaped diagnostic escaped into public observation"
            );
            let ascii_json: String = escaped[1..escaped.len() - 1]
                .chars()
                .map(|character| {
                    if character.is_ascii() {
                        character.to_string()
                    } else {
                        format!("\\u{:04x}", u32::from(character))
                    }
                })
                .collect();
            assert!(
                !public.contains(&ascii_json),
                "ASCII JSON diagnostic escaped into public observation"
            );
            for fragment in sentinel.lines().filter(|fragment| !fragment.is_empty()) {
                assert!(
                    !public.contains(fragment),
                    "multiline diagnostic fragment escaped into public observation"
                );
            }
        }
    }
}
