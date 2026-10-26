-- P1 preview-only projection. P2 owns publication/BOM/stock, never fixture publication.
CREATE TABLE catalogue_items (
    sku text PRIMARY KEY CHECK (sku ~ '^DEMO-[A-Z0-9-]{1,60}$'),
    slug text UNIQUE NOT NULL CHECK (slug ~ '^[a-z0-9]+(-[a-z0-9]+)*$'),
    minor_units bigint NOT NULL CHECK (minor_units >= 0),
    currency text NOT NULL CHECK (currency = 'EUR'),
    fill text NOT NULL CHECK (fill IN ('covers-only', 'with-foam')),
    fixture boolean NOT NULL DEFAULT true CHECK (fixture)
);
CREATE TABLE catalogue_translations (
    sku text NOT NULL REFERENCES catalogue_items(sku),
    locale text NOT NULL CHECK (locale IN ('nl','de','fr','en','ar','tr','kmr')),
    title text NOT NULL CHECK (length(title) BETWEEN 1 AND 160),
    description text NOT NULL CHECK (length(description) BETWEEN 1 AND 2000),
    PRIMARY KEY (sku, locale)
);
