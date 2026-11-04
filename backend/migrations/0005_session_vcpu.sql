-- NULL names an unrecorded allocation; existing sessions cannot be priced from current config.
ALTER TABLE sessions ADD COLUMN vcpu REAL CHECK (vcpu > 0);
