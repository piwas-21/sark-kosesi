-- Add an unseeded read-only review capability; no role or grant is installed.
ALTER TABLE staff_capabilities DROP CONSTRAINT staff_capabilities_capability_check;
ALTER TABLE staff_capabilities ADD CONSTRAINT staff_capabilities_capability_check CHECK(capability IN
 ('catalogue.manage','catalogue.review','warehouse.manage','market.manage','market.review',
  'promotion.manage','promotion.review','payment.reconcile','stock.review','stock.recover',
  'refund.prepare','refund.execution.prepare','reversal.review'));
