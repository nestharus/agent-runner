#!/usr/bin/python3
"""Resident ACP peer with an unresolved prior insertion and a later refusal.
Optionally declares live reattachment, never dedup. A later refusal applies to that request;
it does not report the prior insertion's turn ended. No idle is emitted.
"""
import json, os, sys
from pathlib import Path
base=Path(sys.argv[1]); seq=0
wire=(base/'wire.jsonl').open('w')
def log(direction,value):
    wire.write(json.dumps({'direction':direction,'value':value})+'\n');wire.flush()
def send(v):
    log('send',v);sys.stdout.write(json.dumps(v)+'\n');sys.stdout.flush()
for line in sys.stdin:
    q=json.loads(line);log('recv',q);m=q.get('method');i=q.get('id');p=q.get('params',{})
    if m=='initialize':
        send({'jsonrpc':'2.0','id':i,'result':{'protocolVersion':2,'info':{'name':'uncertain-rejection-peer','version':'fixture'},
            'capabilities':{'session':{}},'_meta':{} if '--no-reattach' in sys.argv else {'oulipoly.ai/liveReattach':{'version':1}}}})
    elif m=='session/new':send({'jsonrpc':'2.0','id':i,'result':{'sessionId':'still-open'}})
    elif m=='session/resume':send({'jsonrpc':'2.0','id':i,'result':{}})
    elif m=='session/prompt':
        seq+=1
        if seq==1:
            (base/'prior-insertion.json').write_text(json.dumps({'pid_in_work':os.getpid(),'request':q,'inserted':True,'ack_sent':False,'turn_end_sent':False})+'\n')
            # Deliberately leaves the prior turn's state unresolved to the owner.
        else:
            send({'jsonrpc':'2.0','id':i,'error':{'code':-32011,'message':'current admission refused; no statement about earlier request'}})
