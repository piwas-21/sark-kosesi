-- This capability is empty until explicitly granted; no account or session is seeded.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review','promotion.manage','promotion.review','payment.reconcile','stock.review'));
-- Explicit collation makes the keyset ordering independent of database locale.
CREATE INDEX late_stock_review_inbox_id_c ON late_stock_review_cases (id COLLATE "C");
