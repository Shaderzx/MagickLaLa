CREATE TABLE IF NOT EXISTS webhook_deliveries (
    webhook_id TEXT NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    delivery_key TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    PRIMARY KEY (webhook_id, delivery_key)
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_received_at ON webhook_deliveries(received_at);

ALTER TABLE agents ADD COLUMN autostart INTEGER NOT NULL DEFAULT 0;

UPDATE agents SET autostart = 1 WHERE enabled = 1 AND pinned_release_id IS NOT NULL;