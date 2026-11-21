ALTER TABLE agents ADD COLUMN IF NOT EXISTS autostart BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE agents SET autostart = TRUE WHERE enabled AND pinned_release_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    webhook_id UUID NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    delivery_key TEXT NOT NULL,
    received_at BIGINT NOT NULL,
    PRIMARY KEY (webhook_id, delivery_key)
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_received_at ON webhook_deliveries(received_at);