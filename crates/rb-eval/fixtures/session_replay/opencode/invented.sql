CREATE TABLE session (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    directory TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL
);
CREATE TABLE message (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE part (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    time_created INTEGER NOT NULL,
    time_updated INTEGER NOT NULL,
    data TEXT NOT NULL
);

INSERT INTO session VALUES (
    'ses_invented_opencode',
    'project-invented-river',
    '/home/riley/invented-river',
    1769940000000,
    1769940360000
);
INSERT INTO session VALUES (
    'ses_invented_missing_project',
    '',
    '/home/invented/missing-project',
    1769940400000,
    1769940460000
);

INSERT INTO message VALUES (
    'msg_missing_project', 'ses_invented_missing_project', 1769940400000, 1769940400000,
    '{"role":"user"}'
);
INSERT INTO part VALUES (
    'part_missing_project', 'msg_missing_project', 'ses_invented_missing_project', 1769940400000, 1769940400000,
    '{"type":"text","text":"Invented row for missing-project rejection accounting."}'
);

INSERT INTO message VALUES (
    'msg_user_early', 'ses_invented_opencode', 1769940000000, 1769940000000,
    '{"role":"user"}'
);
INSERT INTO part VALUES (
    'part_user_early', 'msg_user_early', 'ses_invented_opencode', 1769940000000, 1769940000000,
    '{"type":"text","text":"We decided the invented River queue uses durable mode. owner: Riley Sample company: Example Workshop email riley@river.test."}'
);

INSERT INTO message VALUES (
    'msg_assistant_conflict', 'ses_invented_opencode', 1769940060000, 1769940060000,
    '{"role":"assistant"}'
);
INSERT INTO part VALUES (
    'part_assistant_conflict', 'msg_assistant_conflict', 'ses_invented_opencode', 1769940060000, 1769940060000,
    '{"type":"text","text":"The invented River queue uses volatile mode."}'
);

INSERT INTO message VALUES (
    'msg_assistant_tool', 'ses_invented_opencode', 1769940120000, 1769940120000,
    '{"role":"assistant"}'
);
INSERT INTO part VALUES (
    'part_assistant_tool', 'msg_assistant_tool', 'ses_invented_opencode', 1769940120000, 1769940120000,
    '{"type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"git show HEAD:invented-settings.toml"},"output":"fixture output mode=durable host=db.internal"}}'
);

INSERT INTO message VALUES (
    'msg_reasoning', 'ses_invented_opencode', 1769940180000, 1769940180000,
    '{"role":"assistant"}'
);
INSERT INTO part VALUES (
    'part_reasoning', 'msg_reasoning', 'ses_invented_opencode', 1769940180000, 1769940180000,
    '{"type":"reasoning","text":"Invented private reasoning must not be captured."}'
);

INSERT INTO message VALUES (
    'msg_user_later', 'ses_invented_opencode', 1769940240000, 1769940240000,
    '{"role":"user"}'
);
INSERT INTO part VALUES (
    'part_user_later', 'msg_user_later', 'ses_invented_opencode', 1769940240000, 1769940240000,
    '{"type":"text","text":"Remind me what we decided earlier about the invented River queue."}'
);

INSERT INTO message VALUES (
    'msg_assistant_later', 'ses_invented_opencode', 1769940300000, 1769940300000,
    '{"role":"assistant"}'
);
INSERT INTO part VALUES (
    'part_assistant_later', 'msg_assistant_later', 'ses_invented_opencode', 1769940300000, 1769940300000,
    '{"type":"text","text":"The user statement and committed fixture both say durable mode."}'
);
