"""Ordered fixture regression, not a production receipt/ownership experiment."""
import json
import os
import pathlib
import runpy
import sys

root = pathlib.Path(sys.argv[1])
sys.path.insert(0, str(root))
provider = runpy.run_path(str(root / 'external-provider.py'), run_name='fixture_import')
import slow_control

control = json.loads(slow_control.CONTROL.read_text())
request = {'params': {'turn_projection': 'user_observation', 'after_token': 's11-anchor:0',
                      'max_turns': 1}}
# Explicit ordering: append completes before a nonterminal read is called.
prompt = 'actual appended notification'
(root / 'work' / 'resume-prompts.jsonl').write_text(json.dumps(prompt) + '\n')
slow_control.terminal_target = lambda: None
hidden = slow_control.read_page(request, provider['session_turn_page'])['result']
assert hidden['turns'] == [], hidden
assert hidden['resume_token'] == 's11-anchor:0', hidden
assert hidden['snapshot_id'] == 's11-hidden:s11-anchor:0', hidden
# Model the shared reader persisting exactly the returned checkpoint, then
# handing it to terminal inspection. No test-side rewind to the original anchor.
request['params']['after_token'] = hidden['resume_token']
slow_control.terminal_target = lambda: {'unit_fixture': True}
slow_control.receipt_snapshot = lambda *_: {'unit_fixture': 'no receipt database'}
visible = slow_control.read_page(request, provider['session_turn_page'])['result']
assert visible['turns'][0]['turn_id'] == 's11-observed-user-1', visible
assert visible['resume_token'] == 's11-anchor:1', visible
print(json.dumps({'ordering': 'append -> nonterminal -> persist checkpoint -> terminal',
                  'hidden': hidden, 'visible': visible}))
