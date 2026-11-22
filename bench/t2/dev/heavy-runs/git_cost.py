"""What the git arms (A, A2) spent on conflicts in the heavy runs.

Usage: python3 git_cost.py TRANSCRIPTS_DIR [OUT.json]
TRANSCRIPTS_DIR holds one directory per run with agent-<k>.jsonl (not published: host paths).
Run files are read from the directories beside this script.
Arm A: everything in an agent's second invocation (the hand-back) is conflict work.
Arm A2: everything after the first `t2-integrate` that reported a conflict is conflict work;
the Claude cost of that part is the agent's cost times its share of price-weighted tokens.
"""
import json,glob,os,sys,collections,datetime,statistics as st
HERE=os.path.dirname(os.path.abspath(__file__));TR=sys.argv[1]
RF=sorted(glob.glob(HERE+'/*/*.json'))
W=dict(i=1,cw=2.0,cr=0.1,o=5)   # relative price weights (1h cache write = 2x)
def ts(s): return datetime.datetime.fromisoformat(s.replace('Z','+00:00')).timestamp()
def claude(f):
    """sessions: list of dict(cost, dur, msgs=[(ts,weighted,is_tool)], tools=[(ts,cmd,result)])"""
    ses=[];cur=None;prev=0.0
    for l in open(f):
        o=json.loads(l);t=o.get('type')
        if t=='system' and o.get('subtype')=='init':
            cur=dict(msgs={},tools={},order=[],cost=0,dur=0);ses.append(cur)
        elif t=='assistant':
            m=o['message'];u=m.get('usage',{})
            w=u.get('input_tokens',0)+W['cw']*u.get('cache_creation_input_tokens',0)+W['cr']*u.get('cache_read_input_tokens',0)+W['o']*u.get('output_tokens',0)
            cur['msgs'][m['id']]=(ts(o['timestamp']),w)
            for c in m['content']:
                if c['type']=='tool_use':
                    cur['tools'][c['id']]=[ts(o['timestamp']),c['name'],json.dumps(c['input']),None];cur['order'].append(c['id'])
        elif t=='user':
            c=o['message']['content']
            if isinstance(c,list):
                for x in c:
                    if x.get('type')=='tool_result' and x['tool_use_id'] in cur['tools']:
                        r=x.get('content');r=r if isinstance(r,str) else json.dumps(r)
                        cur['tools'][x['tool_use_id']][3]=r;cur['tools'][x['tool_use_id']].append(ts(o['timestamp']))
        elif t=='result':
            cur['cost']=o['total_cost_usd']-prev;prev=o['total_cost_usd'];cur['dur']=o['duration_ms']/1000
    return ses
def codex(f):
    ses=[];cur=None;prev=None
    for l in open(f):
        o=json.loads(l);t=o.get('type')
        if t=='thread.started': cur=dict(items=[],tok=0);ses.append(cur)
        elif t=='item.completed' and o['item']['type'] in('command_execution','file_change'):
            it=o['item'];cur['items'].append((it['type'],it.get('command',''),str(it.get('aggregated_output',''))))
        elif t=='turn.completed':
            u=o['usage'];tot=u['input_tokens']+u['output_tokens']
            cur['tok']=tot-(prev or 0) if len(ses)>1 else tot;prev=tot
    return ses
rows=[]
for rf in RF:
    d=json.load(open(rf));arm=d['arm']
    if arm not in('A','A2'): continue
    run=d['run'];series=rf.split('/')[-2];T=f'{TR}/{run}'
    harn={a['k']:a['harness'] for a in d['agents']};tid={a['k']:a['id'].split('-')[0] for a in d['agents']}
    cost=d['cost_usd'];R=dict(run=run,arm=arm,series=series,cost=cost,wall=d['wall_s'],tools=d['tool_calls'],tokens=d['tokens_in']+d['tokens_out'])
    cc=0.0;ctools=0;ctok_codex=0;n_conf=0;n_back=0;ctime=0.0;who=[];first_phase=[]
    for k in harn:
        f=f'{T}/agent-{k}.jsonl'
        if harn[k]=='claude':
            S=claude(f)
            if arm=='A':
                first_phase.append(S[0]['dur'])
                for s in S[1:]:
                    cc+=s['cost'];ctools+=len(s['order']);ctime+=s['dur'];n_back+=1
            else:
                # first integrate that reported a conflict
                t0=None;nint=0
                s=S[0];tc=None
                for id_ in s['order']:
                    tt,name,inp,res=s['tools'][id_][:4]
                    if 't2-integrate' in inp:
                        nint+=1
                        if tc is None and res and ('CONFLICT' in res or 'conflict' in res.lower()):
                            tc=s['tools'][id_][4] if len(s['tools'][id_])>4 else tt
                if tc is not None:
                    tot=sum(w for _,w in s['msgs'].values());post=sum(w for t,w in s['msgs'].values() if t>=tc)
                    cc+=s['cost']*post/tot;ctools+=sum(1 for i in s['order'] if s['tools'][i][0]>=tc)
                    last=max(t for t,_ in s['msgs'].values());ctime+=last-tc;n_back+=1
        else:
            S=codex(f)
            if arm=='A':
                for s in S[1:]: ctools+=len(s['items']);ctok_codex+=s['tok'];n_back+=1
            else:
                s=S[0];idx=None
                for i,(ty,cmd,out) in enumerate(s['items']):
                    if 't2-integrate' in cmd and idx is None and 'conflict' in out.lower(): idx=i
                if idx is not None:
                    ctools+=len(s['items'])-idx-1;n_back+=1
                    ctok_codex+=s['tok']*(len(s['items'])-idx-1)/max(1,len(s['items']))
    if arm=='A':
        n_conf=sum(1 for m in d['merges'] if m['conflict'])
        files=collections.Counter(p for m in d['merges'] for p in m['conflict_files'])
        who=[tid[m['k']] for m in d['merges'] if m['conflict']]
        allclaude=all(h=='claude' for h in harn.values())
        R['end_phase_s']=round(d['wall_s']-max(first_phase),0) if allclaude else None
    else:
        n_conf=d['integrate_conflicts']
        who=[tid[int(k)] for k,v in d['integrations'].items() if any(x['status']=='conflict' for x in v)]
        files={}
        R['end_phase_s']=None
    R.update(conflicts=n_conf,agents_fixing=n_back,fix_cost=round(cc,3),fix_cost_share=round(cc/cost,3) if cost else None,fix_tools=ctools,fix_tool_share=round(ctools/d['tool_calls'],3),fix_time_sum_claude=round(ctime),who=who,files=dict(files),lost_lines=d['lost_lines']['total'],
             tasks=d['tasks_on_main'],lost=[k.split('-')[0] for k,v in d['done'].items() if not v],integ=[k.split('-')[0] for k,v in d['integration'].items() if not v],codex_fix_tok=round(ctok_codex))
    rows.append(R)
if len(sys.argv)>2: json.dump(rows,open(sys.argv[2],'w'),indent=1)
for r in rows:
    print(f"{r['arm']:2} {r['run'].split('-')[3]:4} {r['series']:12} conf={r['conflicts']:2} fixing={r['agents_fixing']:2} fix$={r['fix_cost']:.2f}/{r['cost']:.2f} ({r['fix_cost_share']:.0%}) fixtools={r['fix_tools']:3}/{r['tools']} ({r['fix_tool_share']:.0%}) end_s={r['end_phase_s']} wall={r['wall']:.0f} lostlines={r['lost_lines']:2} lost={r['lost']} integ={r['integ']}")
print()
for arm in('A','A2'):
    X=[r for r in rows if r['arm']==arm]
    print(arm,'runs',len(X),'mean conflicts',round(st.mean(r['conflicts'] for r in X),1),'agents fixing',round(st.mean(r['agents_fixing'] for r in X),1),
          'fix cost share (pooled)',round(sum(r['fix_cost'] for r in X)/sum(r['cost'] for r in X),3),'fix tool share',round(sum(r['fix_tools'] for r in X)/sum(r['tools'] for r in X),3),
          'who',collections.Counter(w for r in X for w in r['who']).most_common(), 'files',collections.Counter(f for r in X for f in r['files']).most_common(6))
