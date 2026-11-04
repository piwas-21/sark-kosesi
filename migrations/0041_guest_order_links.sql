-- Private guest-order ownership custody. Bearer possession and fresh server
-- clocks are checked by the trusted Rust caller; no bearer or buyer PII is copied.
CREATE TABLE guest_order_link_proofs (
    verifier bytea PRIMARY KEY CHECK (octet_length(verifier)=32),
    order_id text NOT NULL REFERENCES checkout_orders(id),
    basket_id text NOT NULL REFERENCES guest_baskets(id),
    created_at bigint NOT NULL CHECK (created_at>=0),
    expires_at bigint NOT NULL CHECK (expires_at>created_at AND expires_at-created_at=900),
    consumed_at bigint CHECK (consumed_at>=created_at AND consumed_at<expires_at),
    consumed_account_id bigint REFERENCES verified_customer_accounts(id),
    consumed_xid xid8,
    CHECK ((consumed_at IS NULL AND consumed_account_id IS NULL AND consumed_xid IS NULL)
        OR (consumed_at IS NOT NULL AND consumed_account_id IS NOT NULL AND consumed_xid IS NOT NULL))
);

CREATE TABLE customer_order_links (
    order_id text PRIMARY KEY REFERENCES checkout_orders(id),
    account_id bigint NOT NULL REFERENCES verified_customer_accounts(id),
    proof_verifier bytea NOT NULL UNIQUE REFERENCES guest_order_link_proofs(verifier),
    linked_at bigint NOT NULL CHECK (linked_at>=0),
    linked_xid xid8 NOT NULL
);

CREATE FUNCTION guard_guest_order_link_proof() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    original checkout_orders%ROWTYPE;
    buyer_email text;
    account_email text;
    at_position integer;
BEGIN
    IF TG_OP='DELETE' THEN
        RAISE EXCEPTION 'Guest order link proofs cannot be deleted' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation')<>'read committed' THEN
        RAISE EXCEPTION 'Guest order link custody requires read committed' USING ERRCODE='23514';
    END IF;
    IF TG_OP='INSERT' THEN
        IF NEW.consumed_at IS NOT NULL OR NEW.consumed_account_id IS NOT NULL
            OR NEW.consumed_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid guest order link proof insertion' USING ERRCODE='23514';
        END IF;
    ELSE
        IF (NEW.verifier,NEW.order_id,NEW.basket_id,NEW.created_at,NEW.expires_at)
            IS DISTINCT FROM (OLD.verifier,OLD.order_id,OLD.basket_id,OLD.created_at,OLD.expires_at)
            OR OLD.consumed_at IS NOT NULL OR OLD.consumed_account_id IS NOT NULL
            OR OLD.consumed_xid IS NOT NULL OR NEW.consumed_at IS NULL
            OR NEW.consumed_account_id IS NULL OR NEW.consumed_xid IS NOT NULL THEN
            RAISE EXCEPTION 'Invalid guest order link proof transition' USING ERRCODE='23514';
        END IF;
    END IF;

    -- The proof is locked before this order on redemption. Every competing
    -- proof uses the same order lock, so an existing link is observed after waits.
    SELECT * INTO original FROM checkout_orders WHERE id=NEW.order_id FOR UPDATE;
    IF original.id IS NULL OR original.basket_id IS DISTINCT FROM NEW.basket_id
        OR NEW.created_at<original.created_at
        OR EXISTS (SELECT 1 FROM customer_order_links WHERE order_id=NEW.order_id) THEN
        RAISE EXCEPTION 'Guest order link proof scope mismatch' USING ERRCODE='23514';
    END IF;
    IF TG_OP='UPDATE' THEN
        SELECT email_identity INTO account_email FROM verified_customer_accounts
            WHERE id=NEW.consumed_account_id AND verified_at<=NEW.consumed_at;
        IF jsonb_typeof(original.snapshot->'buyer'->'email') IS DISTINCT FROM 'string' THEN
            RAISE EXCEPTION 'Guest order link mailbox mismatch' USING ERRCODE='23514';
        END IF;
        buyer_email := original.snapshot->'buyer'->>'email';
        at_position := strpos(buyer_email,'@');
        -- Only ASCII is compared, with one @, a case-sensitive local part, and
        -- explicit ASCII domain folding. Rust applies the stricter identity policy.
        IF account_email IS NULL OR octet_length(buyer_email)<>char_length(buyer_email)
            OR at_position<=1 OR at_position>=char_length(buyer_email)
            OR char_length(buyer_email)-char_length(replace(buyer_email,'@',''))<>1
            OR (substring(buyer_email FROM 1 FOR at_position-1) || '@' ||
                translate(substring(buyer_email FROM at_position+1),
                    'ABCDEFGHIJKLMNOPQRSTUVWXYZ','abcdefghijklmnopqrstuvwxyz')) COLLATE "C"
                IS DISTINCT FROM account_email COLLATE "C" THEN
            RAISE EXCEPTION 'Guest order link mailbox mismatch' USING ERRCODE='23514';
        END IF;
        NEW.consumed_xid := pg_current_xact_id();
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_guest_order_link_proof BEFORE INSERT OR UPDATE OR DELETE ON guest_order_link_proofs
    FOR EACH ROW EXECUTE FUNCTION guard_guest_order_link_proof();

CREATE FUNCTION guard_customer_order_link() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP<>'INSERT' THEN
        RAISE EXCEPTION 'Customer order links are immutable' USING ERRCODE='23514';
    END IF;
    IF current_setting('transaction_isolation')<>'read committed' OR NEW.linked_xid IS NOT NULL THEN
        RAISE EXCEPTION 'Invalid customer order link insertion' USING ERRCODE='23514';
    END IF;
    -- Only this transaction's consumption can create the reciprocal link.
    IF NOT EXISTS (SELECT 1 FROM guest_order_link_proofs p
        WHERE p.verifier=NEW.proof_verifier AND p.order_id=NEW.order_id
            AND p.consumed_account_id=NEW.account_id AND p.consumed_at=NEW.linked_at
            AND p.consumed_xid=pg_current_xact_id()) THEN
        RAISE EXCEPTION 'Customer order link proof mismatch' USING ERRCODE='23514';
    END IF;
    NEW.linked_xid := pg_current_xact_id();
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_customer_order_link BEFORE INSERT OR UPDATE OR DELETE ON customer_order_links
    FOR EACH ROW EXECUTE FUNCTION guard_customer_order_link();

-- Neither half can commit alone, including when constraints are forced early.
CREATE FUNCTION require_guest_order_link() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_TABLE_NAME='guest_order_link_proofs' THEN
        IF NEW.consumed_at IS NOT NULL AND NOT EXISTS (
            SELECT 1 FROM customer_order_links l WHERE l.order_id=NEW.order_id
                AND l.account_id=NEW.consumed_account_id AND l.proof_verifier=NEW.verifier
                AND l.linked_at=NEW.consumed_at AND l.linked_xid=NEW.consumed_xid
        ) THEN
            RAISE EXCEPTION 'Consumed guest order link proof requires its link' USING ERRCODE='23514';
        END IF;
    ELSIF NOT EXISTS (
        SELECT 1 FROM guest_order_link_proofs p WHERE p.verifier=NEW.proof_verifier
            AND p.order_id=NEW.order_id AND p.consumed_account_id=NEW.account_id
            AND p.consumed_at=NEW.linked_at AND p.consumed_xid=NEW.linked_xid
    ) THEN
        RAISE EXCEPTION 'Customer order link requires its consumed proof' USING ERRCODE='23514';
    END IF;
    RETURN NULL;
END;
$$;
CREATE CONSTRAINT TRIGGER require_guest_order_link AFTER UPDATE ON guest_order_link_proofs
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_order_link();
CREATE CONSTRAINT TRIGGER require_guest_order_link AFTER INSERT ON customer_order_links
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION require_guest_order_link();
