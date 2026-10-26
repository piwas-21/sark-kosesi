-- Isolated CI/local seed only. All values invented, visibly labelled, not merchant facts.
INSERT INTO catalogue_items (sku, slug, minor_units, currency, fill, fixture)
VALUES ('DEMO-COURTYARD-COVERS', 'courtyard-demo', 12345, 'EUR', 'covers-only', true);
INSERT INTO catalogue_translations (sku,locale,title,description,care)
SELECT 'DEMO-COURTYARD-COVERS', locale, 'Demonstration seating',
'Test fixture. Not a real product or offer.', 'Demonstration only; care instructions pending.'
FROM unnest(ARRAY['nl','de','fr','en','ar','tr','kmr']) AS locale;
