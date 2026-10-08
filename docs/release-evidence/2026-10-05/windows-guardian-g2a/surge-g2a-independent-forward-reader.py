"""Read-only inverse framing check, independent of Rust and the freeze encoder."""
import hashlib
import json
import re
from pathlib import Path

root = Path('/Users/vanyastafford/Develop/surge')
p = root / 'crates/surge-core/src/execution_recovery/guardian_ledger/literal_vectors.json'
vectors = json.loads(p.read_text())
assert len(vectors) == 14
alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
layouts = {
    'authority': [('installation_id', 'hex32'), ('executable_hash', 'hash'), ('protocol_version', 'u4')],
    'process': [('pid', 'u4'), ('creation_filetime', 'u8')],
    'binding': [('authority', 'authority'), ('occurrence', 'ulid'), ('guardian', 'process'), ('endpoint', 'text')],
    'context': [('run', 'ulid'), ('invocation', 'ulid'), ('occurrence', 'ulid'), ('authority', 'authority')],
    'launch_intent': [('host', 'process'), ('registry_generation', 'u8')],
    'cancelled_before_spawn': [('launch_operation', 'hex16'), ('host', 'process'), ('registry_generation', 'u8'), ('revocation_generation', 'u8')],
    'guardian_bound': [('binding', 'binding'), ('host', 'process'), ('registry_generation', 'u8')],
    'lease_taken_over': [('binding', 'binding'), ('previous_lease', 'u8'), ('previous_host', 'process'), ('host', 'process'), ('previous_registry_generation', 'u8'), ('registry_generation', 'u8'), ('authorization', 'auth')],
    'child_established': [('container', 'container')],
    'resume_authorized': [('container', 'container'), ('barrier_generation', 'u8')],
    'resume_observed': [('container', 'container'), ('resume_operation', 'hex16'), ('barrier_generation', 'u8'), ('outcome', 'outcome')],
    'settlement_requested': [('binding', 'binding'), ('barrier_generation', 'u8')],
    'settlement_observed': [('settlement_operation', 'hex16'), ('proof', 'proof')],
    'settlement_consumed': [('settlement_operation', 'hex16'), ('settlement_hash', 'hash'), ('settlement_sequence', 'u8'), ('barrier_generation', 'u8'), ('query_generation', 'u8')],
    'previous_host_terminated': [('host', 'process'), ('lease', 'u8'), ('observation_generation', 'u8')],
    'previous_host_relinquished': [('host', 'process'), ('lease', 'u8'), ('operation', 'hex16'), ('hash', 'hash')],
    'started': [('previous_suspend_count', 'u4')],
    'failed': [('win32_error', 'u4')],
    'uncertain': [],
    'bound_job_empty': [('binding', 'binding'), ('barrier_generation', 'u8'), ('query_generation', 'u8')],
    'child_job_empty': [('container', 'container'), ('barrier_generation', 'u8'), ('query_generation', 'u8')],
}
tags = {
    'body': ['launch_intent', 'cancelled_before_spawn', 'guardian_bound', 'lease_taken_over', 'child_established', 'resume_authorized', 'resume_observed', 'settlement_requested', 'settlement_observed', 'settlement_consumed'],
    'auth': ['previous_host_terminated', 'previous_host_relinquished'],
    'outcome': ['started', 'failed', 'uncertain'],
    'proof': ['bound_job_empty', 'child_job_empty'],
}
seen = {key: set() for key in tags}


class Reader:
    def __init__(self, data):
        self.data = data
        self.pos = 0

    def take(self, count):
        assert self.pos + count <= len(self.data)
        value = self.data[self.pos:self.pos + count]
        self.pos += count
        return value

    def num(self, count):
        return int.from_bytes(self.take(count), 'big')

    def verify(self, kind, value):
        if kind.startswith('u') and kind[1:].isdigit():
            assert self.num(int(kind[1:])) == value
        elif kind.startswith('hex'):
            assert self.take(int(kind[3:])).hex() == value
        elif kind == 'hash':
            assert 'sha256:' + self.take(32).hex() == value
        elif kind == 'ulid':
            number, text = self.num(16), ''
            for _ in range(26):
                text = alphabet[number & 31] + text
                number >>= 5
            assert number == 0 and text == value
        elif kind == 'text':
            length = self.num(4)
            assert length == 106
            assert self.take(length).decode('ascii') == value
        elif kind in tags:
            tag = self.num(1)
            assert 1 <= tag <= len(tags[kind])
            variant = tags[kind][tag - 1]
            assert variant == value['kind']
            seen[kind].add(tag)
            self.verify(variant, value)
        elif kind == 'container':
            assert self.num(1) == 1 and value['kind'] == 'windows_guardian'
            assert self.num(4) == 1 and value['version'] == 1
            self.verify('binding', value['identity'])
            self.verify('process', value['identity']['child'])
            assert self.num(1) == 1 and value['coverage'] == 'group_only'
        else:
            for key, field_type in layouts[kind]:
                self.verify(field_type, value[key])
            if kind == 'binding':
                assert value['endpoint'] == 'surge-guardian-' + value['authority']['installation_id'] + '-' + value['occurrence'].lower()


for fixture in vectors:
    record = fixture['record']
    preimage = bytes.fromhex(fixture['preimage'])
    reader = Reader(preimage)
    prefix = b'surge.windows-guardian-ledger.v1\x00'
    assert reader.take(len(prefix)) == prefix
    reader.verify('u4', record['version'])
    assert record['version'] == 1
    reader.verify('context', record['context'])
    reader.verify('hex16', record['operation'])
    for key, field_type in [('predecessor', 'hash'), ('lease', 'u8')]:
        assert key in record
        reader.verify('u1', int(record[key] is not None))
        if record[key] is not None:
            reader.verify(field_type, record[key])
    reader.verify('body', record['body'])
    assert reader.pos == len(preimage)
    assert hashlib.sha256(preimage).hexdigest() == fixture['sha256']
    print(fixture['name'], len(preimage), 'full-field framing/digest PASS')

for category, variants in tags.items():
    assert seen[category] == set(range(1, len(variants) + 1))
by_name = {fixture['name']: fixture for fixture in vectors}
assert len(by_name) == 14
old = Path('/tmp/surge-g2a-independent-literal-oracles.md').read_text()
for section in old.split('\n## ')[1:]:
    fixture = by_name[section.splitlines()[0]]
    assert fixture['record'] == json.loads(re.search(r'```json\n(.*?)\n```', section, re.S).group(1))
    assert fixture['preimage'] == ''.join(re.search(r'```text\n(.*?)\n```', section, re.S).group(1).split())
    assert fixture['sha256'] == re.search(r'SHA-256: `([^`]+)`', section).group(1)
assert by_name['genesis']['preimage'] == Path('/tmp/surge-g2a-genesis.hex').read_text().strip()
assert by_name['genesis']['sha256'] == 'f0d2842157aefb73553b44878cd654aeb0382fd0084bd855c40e7709e576fe8a'
assert by_name['consumed']['record']['body']['settlement_hash'] == 'sha256:' + by_name['bound_job_empty']['sha256']
print('COVERAGE', {key: sorted(value) for key, value in seen.items()})
print('4 known oracles unchanged; consumed references complete observed-record digest PASS')
print('VECTOR FILE SHA256', hashlib.sha256(p.read_bytes()).hexdigest())
