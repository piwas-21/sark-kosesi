-- Expand the preview catalogue. No merchant facts or publication path are added.
CREATE TABLE product_families (
    code text PRIMARY KEY CHECK (code ~ '^DEMO-[A-Z0-9-]{1,60}$'),
    fixture boolean NOT NULL DEFAULT true CHECK (fixture)
);
CREATE TABLE catalogue_variants (
    sku text PRIMARY KEY REFERENCES catalogue_items(sku),
    family_code text NOT NULL REFERENCES product_families(code),
    size_code text NOT NULL CHECK (length(btrim(size_code)) BETWEEN 1 AND 80),
    textile_code text NOT NULL CHECK (length(btrim(textile_code)) BETWEEN 1 AND 80),
    -- Prepacked kits need a separate explicit conversion policy before use.
    stock_policy text NOT NULL DEFAULT 'separable' CHECK (stock_policy = 'separable')
);
CREATE TABLE physical_components (
    code text PRIMARY KEY CHECK (code ~ '^DEMO-[A-Z0-9-]{1,60}$'),
    kind text NOT NULL CHECK (kind IN ('cover', 'foam')),
    fixture boolean NOT NULL DEFAULT true CHECK (fixture)
);
CREATE TABLE catalogue_boms (
    sku text NOT NULL REFERENCES catalogue_variants(sku),
    revision integer NOT NULL CHECK (revision > 0),
    sealed boolean NOT NULL DEFAULT false,
    PRIMARY KEY (sku, revision)
);
CREATE TABLE catalogue_bom_lines (
    sku text NOT NULL,
    revision integer NOT NULL,
    component_code text NOT NULL REFERENCES physical_components(code),
    quantity integer NOT NULL CHECK (quantity BETWEEN 1 AND 10000),
    PRIMARY KEY (sku, revision, component_code),
    FOREIGN KEY (sku, revision) REFERENCES catalogue_boms(sku, revision)
);

-- A line edit and sealing serialize on the same parent. A sealed BOM is an
-- immutable stock/order input; changes require a new revision, never an edit.
CREATE FUNCTION guard_bom_line() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND (NEW.sku, NEW.revision) IS DISTINCT FROM (OLD.sku, OLD.revision) THEN
        RAISE EXCEPTION 'BOM line identity is immutable' USING ERRCODE = '23514';
    END IF;
    IF TG_OP = 'DELETE' THEN
        UPDATE catalogue_boms SET sealed = sealed
        WHERE sku = OLD.sku AND revision = OLD.revision;
    ELSE
        UPDATE catalogue_boms SET sealed = sealed
        WHERE sku = NEW.sku AND revision = NEW.revision;
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_bom_line BEFORE INSERT OR UPDATE OR DELETE ON catalogue_bom_lines
FOR EACH ROW EXECUTE FUNCTION guard_bom_line();

CREATE FUNCTION guard_bom_revision() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE item_fill text; covers bigint; foams bigint;
BEGIN
    IF TG_OP <> 'INSERT' THEN
        IF OLD.sealed THEN
            RAISE EXCEPTION 'Sealed BOM is immutable' USING ERRCODE = '23514';
        END IF;
        IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
        IF (NEW.sku, NEW.revision) IS DISTINCT FROM (OLD.sku, OLD.revision) THEN
            RAISE EXCEPTION 'BOM identity is immutable' USING ERRCODE = '23514';
        END IF;
    END IF;
    IF NEW.sealed THEN
        -- The no-op writes here and in guard_bom_line also create MVCC conflicts
        -- under REPEATABLE READ, preventing sealing/fill checks on stale snapshots.
        UPDATE catalogue_items SET fill = fill WHERE sku = NEW.sku RETURNING fill INTO item_fill;
        SELECT count(*) FILTER (WHERE c.kind = 'cover'), count(*) FILTER (WHERE c.kind = 'foam')
        INTO covers, foams FROM catalogue_bom_lines l JOIN physical_components c ON c.code = l.component_code
        WHERE l.sku = NEW.sku AND l.revision = NEW.revision;
        IF covers = 0 OR covers + foams > 64 OR (item_fill = 'covers-only' AND foams <> 0) OR (item_fill = 'with-foam' AND foams = 0) THEN
            RAISE EXCEPTION 'BOM does not match fill configuration' USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_bom_revision BEFORE INSERT OR UPDATE OR DELETE ON catalogue_boms
FOR EACH ROW EXECUTE FUNCTION guard_bom_revision();

CREATE FUNCTION guard_component_kind() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.kind IS DISTINCT FROM OLD.kind OR NEW.code IS DISTINCT FROM OLD.code THEN
        RAISE EXCEPTION 'Physical component identity is immutable' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_component_kind BEFORE UPDATE ON physical_components
FOR EACH ROW EXECUTE FUNCTION guard_component_kind();

CREATE FUNCTION guard_variant_fill() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.fill IS DISTINCT FROM OLD.fill AND EXISTS (SELECT 1 FROM catalogue_boms WHERE sku = OLD.sku AND sealed) THEN
        RAISE EXCEPTION 'Sealed BOM fixes variant fill' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER guard_variant_fill BEFORE UPDATE ON catalogue_items
FOR EACH ROW EXECUTE FUNCTION guard_variant_fill();
