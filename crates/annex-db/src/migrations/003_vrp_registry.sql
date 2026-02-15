CREATE TABLE vrp_roles (
    role_code INTEGER PRIMARY KEY,
    label TEXT NOT NULL
);

INSERT INTO vrp_roles (role_code, label) VALUES (1, 'HUMAN');
INSERT INTO vrp_roles (role_code, label) VALUES (2, 'AI_AGENT');
INSERT INTO vrp_roles (role_code, label) VALUES (3, 'COLLECTIVE');
INSERT INTO vrp_roles (role_code, label) VALUES (4, 'BRIDGE');
INSERT INTO vrp_roles (role_code, label) VALUES (5, 'SERVICE');

CREATE TABLE vrp_topics (
    topic TEXT PRIMARY KEY,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO vrp_topics (topic, description) VALUES ('annex:server:v1', 'Server identity scope');
INSERT INTO vrp_topics (topic, description) VALUES ('annex:channel:v1', 'Channel identity scope');
INSERT INTO vrp_topics (topic, description) VALUES ('annex:federation:v1', 'Federation identity scope');

CREATE TABLE vrp_identities (
    commitment_hex TEXT PRIMARY KEY,
    role_code INTEGER NOT NULL,
    node_id INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (role_code) REFERENCES vrp_roles(role_code)
);
