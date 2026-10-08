-- Distinguish a complete short sound from a truncated one-second head.
ALTER TABLE preview_cache ADD COLUMN complete INTEGER NOT NULL DEFAULT 0 CHECK (complete IN (0,1));
