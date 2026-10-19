"""Independent reimplementation of mind's BM25 baseline (bench/longmemeval.py
@ 91cbbf0 + mind.py _tokenize/_id), written from reading that source, to score
the same LongMemEval oracle turn records. Usage: bm25_replica.py DATA IDS_JSON|all FORMAT(mind|clean)"""
import hashlib, json, math, re, sys

WORD_RUN = re.compile(r"[\w\u0600-\u06ff]+", re.UNICODE)
# mind also lists CJK ext A (\u3400-\u4dbf) and kana (\u3040-\u30ff); both lie
# inside \u2e80-\u9fff already, so dropping them leaves the set unchanged.
NOSPACE = re.compile("[\u2e80-\u9fff\uf900-\ufaff\uac00-\ud7af\u0e00-\u0e7f]")

def bigrams(chars):
    return ["".join(chars)] if len(chars) < 2 else ["".join(chars[i:i+2]) for i in range(len(chars)-1)]

def tokenize(text):
    out = []
    for run in WORD_RUN.findall(text or ""):
        alpha, nospace = [], []
        for ch in run:
            if NOSPACE.match(ch):
                if alpha:
                    if len(alpha) >= 3: out.append("".join(alpha))
                    alpha = []
                nospace.append(ch)
            else:
                if nospace:
                    out.extend(bigrams(nospace)); nospace = []
                alpha.append(ch)
        if len(alpha) >= 3: out.append("".join(alpha))
        if nospace: out.extend(bigrams(nospace))
    return out

def tie_key(text):
    """mind's content address (md5[:12]), used only to order equal BM25
    scores exactly as mind does. Not a security use."""
    return hashlib.md5(text.encode("utf-8"), usedforsecurity=False).hexdigest()[:12]
def terms(text): return [t.lower() for t in tokenize(text)]
def norm(v):
    if v is None: return ""
    if isinstance(v, (list, tuple)): return " ".join(norm(x) for x in v)
    return " ".join(str(v).lower().split())
def answer_in(a, t): a = norm(a); return bool(a) and a in norm(t)

def records(q, fmt):
    ans = set(map(str, q.get("answer_session_ids", []) or []))
    sids = [str(x) for x in q.get("haystack_session_ids", []) or []]
    dates = q.get("haystack_dates", []) or []
    docs, ev = {}, set()
    for i, turns in enumerate(q.get("haystack_sessions", []) or []):
        sid = sids[i] if i < len(sids) else str(i)
        date = dates[i] if i < len(dates) else ""
        se = sid in ans; marked = any(t.get("has_answer") for t in turns)
        for t in turns:
            role = t.get("role") or "unknown-role"
            text = ("[%s] question %s session %s %s: %s" % (date or "unknown-date", q["question_id"], sid, role, t.get("content", ""))
                    if fmt == "mind" else "%s: %s" % (role, t.get("content", "")))
            nid = tie_key(text); docs[nid] = text
            if t.get("has_answer") or (se and not marked): ev.add(nid)
    return docs, ev

def recall(docs, query, k=5):
    qt = terms(query); D = [(n, t, terms(t)) for n, t in docs.items()]
    if not qt or not D: return []
    N = len(D); avg = (sum(len(x) for _, _, x in D) / N) or 1.0
    df = {w: sum(w in x for _, _, x in D) for w in set(qt)}
    k1, b = 1.5, 0.75; ranked = []
    for n, t, x in D:
        s = 0.0; L = len(x)
        for w in qt:
            f = x.count(w)
            if not f: continue
            idf = math.log(1.0 + (N - df[w] + 0.5) / (df[w] + 0.5))
            s += idf * (f * (k1 + 1.0) / (f + k1 * (1.0 - b + b * L / avg)))
        if s: ranked.append((n, s, t))
    ranked.sort(key=lambda r: (-r[1], r[0]))
    return ranked[:k]

data = json.load(open(sys.argv[1])); by = {q["question_id"]: q for q in data}
ids = ([q["question_id"] for q in data if not q["question_id"].endswith("_abs")] if sys.argv[2] == "all"
       else json.load(open(sys.argv[2]))["evaluated_question_ids"])
fmt = sys.argv[3]; e1 = ek = ak = n = 0; per = {}
for qid in ids:
    q = by[qid]; docs, ev = records(q, fmt)
    if not ev: continue
    top = recall(docs, q.get("question", "")); n += 1
    hit1 = bool(top) and top[0][0] in ev; hitk = any(r[0] in ev for r in top)
    hita = any(answer_in(q.get("answer"), r[2]) for r in top)
    e1 += hit1; ek += hitk; ak += hita; per[qid] = [hit1, hitk, hita]
print(json.dumps({"n": n, "evidence_at_1": e1 / n, "evidence_at_k": ek / n, "answer_string_at_k": ak / n, "per_question": per}))
