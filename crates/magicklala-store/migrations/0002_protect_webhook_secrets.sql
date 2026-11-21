CREATE TRIGGER IF NOT EXISTS credentials_signing_webhooks_are_protected
BEFORE DELETE ON credentials
WHEN EXISTS (SELECT 1 FROM webhooks WHERE secret_credential_id = OLD.id)
BEGIN
    SELECT RAISE(ABORT, 'credential is referenced by a webhook');
END;

CREATE INDEX IF NOT EXISTS execution_runs_by_correlation ON execution_runs (agent_id, json_extract(trace, '$.correlation_id'), started_at);
