-- Private fixed-window admission stores only keyed digests, never addresses or keys.
-- The trusted caller locks the scope before changing counts or clearing targets.
CREATE TABLE customer_credential_admission_scopes (
    purpose smallint PRIMARY KEY CHECK (purpose BETWEEN 1 AND 5),
    window_start bigint,
    last_seen_at bigint,
    key_version integer,
    key_check bytea,
    global_limit integer,
    target_limit integer,
    admitted integer NOT NULL CHECK (admitted>=0),
    CHECK (
        (window_start IS NULL AND last_seen_at IS NULL AND key_version IS NULL
            AND key_check IS NULL AND global_limit IS NULL AND target_limit IS NULL
            AND admitted=0)
        OR
        (window_start IS NOT NULL AND last_seen_at IS NOT NULL AND key_version IS NOT NULL
            AND key_check IS NOT NULL AND global_limit IS NOT NULL AND target_limit IS NOT NULL
            AND window_start BETWEEN 0 AND 9007199254740991
            AND window_start%60=0
            AND last_seen_at BETWEEN window_start AND 9007199254740991
            AND last_seen_at-window_start<60
            AND key_version>=1 AND octet_length(key_check)=32
            AND global_limit BETWEEN 1 AND 1000
            AND target_limit BETWEEN 1 AND 100 AND target_limit<=global_limit
            AND admitted<=global_limit)
    )
);

INSERT INTO customer_credential_admission_scopes (purpose, admitted)
VALUES (1,0), (2,0), (3,0), (4,0), (5,0);

CREATE TABLE customer_credential_admission_targets (
    purpose smallint NOT NULL REFERENCES customer_credential_admission_scopes(purpose),
    target_digest bytea NOT NULL CHECK (octet_length(target_digest)=32),
    admitted integer NOT NULL CHECK (admitted BETWEEN 1 AND 100),
    PRIMARY KEY (purpose, target_digest)
);
