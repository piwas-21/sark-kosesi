# EU destination readiness evidence

Piwas intends to serve all 27 EU countries from Netherlands stock. This register
tracks the evidence needed to decide where delivery and payment can be enabled;
it is not a shipping policy, tax table or statement of applicable law. All 27
rows are **pending** and disabled: no qualified destination evidence has been
recorded here. A language, prototype, fixture or successful source test does not
establish a deliverable destination.

## Register

Each evidence column holds a reference to a completed record using the template
below. `—` means that no reviewed record is recorded. A country can have more
than one record when territory, postcode, product or service terms differ.

| Country     | Code | Status  | Delivery and payment | Territory/postcode | Carrier/returns | VAT/invoicing | Legal/packaging |
| ----------- | ---- | ------- | -------------------- | ------------------ | --------------- | ------------- | --------------- |
| Austria     | AT   | Pending | Disabled             | —                  | —               | —             | —               |
| Belgium     | BE   | Pending | Disabled             | —                  | —               | —             | —               |
| Bulgaria    | BG   | Pending | Disabled             | —                  | —               | —             | —               |
| Croatia     | HR   | Pending | Disabled             | —                  | —               | —             | —               |
| Cyprus      | CY   | Pending | Disabled             | —                  | —               | —             | —               |
| Czechia     | CZ   | Pending | Disabled             | —                  | —               | —             | —               |
| Denmark     | DK   | Pending | Disabled             | —                  | —               | —             | —               |
| Estonia     | EE   | Pending | Disabled             | —                  | —               | —             | —               |
| Finland     | FI   | Pending | Disabled             | —                  | —               | —             | —               |
| France      | FR   | Pending | Disabled             | —                  | —               | —             | —               |
| Germany     | DE   | Pending | Disabled             | —                  | —               | —             | —               |
| Greece      | GR   | Pending | Disabled             | —                  | —               | —             | —               |
| Hungary     | HU   | Pending | Disabled             | —                  | —               | —             | —               |
| Ireland     | IE   | Pending | Disabled             | —                  | —               | —             | —               |
| Italy       | IT   | Pending | Disabled             | —                  | —               | —             | —               |
| Latvia      | LV   | Pending | Disabled             | —                  | —               | —             | —               |
| Lithuania   | LT   | Pending | Disabled             | —                  | —               | —             | —               |
| Luxembourg  | LU   | Pending | Disabled             | —                  | —               | —             | —               |
| Malta       | MT   | Pending | Disabled             | —                  | —               | —             | —               |
| Netherlands | NL   | Pending | Disabled             | —                  | —               | —             | —               |
| Poland      | PL   | Pending | Disabled             | —                  | —               | —             | —               |
| Portugal    | PT   | Pending | Disabled             | —                  | —               | —             | —               |
| Romania     | RO   | Pending | Disabled             | —                  | —               | —             | —               |
| Slovakia    | SK   | Pending | Disabled             | —                  | —               | —             | —               |
| Slovenia    | SI   | Pending | Disabled             | —                  | —               | —             | —               |
| Spain       | ES   | Pending | Disabled             | —                  | —               | —             | —               |
| Sweden      | SE   | Pending | Disabled             | —                  | —               | —             | —               |

## Evidence record template

Create a separate, reviewable record for each distinct scope and place its
reference in the relevant register cells. Evidence may be shared across rows
only when it explicitly covers each destination and the same terms. Keep
commercially sensitive documents in appropriately restricted storage; use
stable references here rather than customer information or credentials.

- **Record:** ID and version; evidence location; destination code; exact
  product/configuration and service scope; supplier, carrier and adviser as
  applicable; evidence date, effective period and next review/expiry date.
- **Decision and custody:** responsible Piwas owner; reviewer and their relevant
  qualification; review date; pending, blocked or ready decision and reasons;
  approval reference; superseded record and withdrawal/review trigger. Missing,
  expired, conflicting or unreviewed evidence stays pending or blocked.
- **Territory and postcodes:** explicit included territory and postcode rules,
  exclusions and exceptions, including special territories, islands or remote
  areas where relevant; evidence source and version. Record unresolved ranges
  as unsupported, not implicitly included. Tie any reviewed rules to the exact
  destination scope.
- **Carrier and returns:** named carrier and bulky service, written service
  coverage and terms for the scoped destination; measured packed product
  dimensions, weight and parcel count, service limits and access/handling
  constraints; delivery cost, estimate and surcharges; return collection,
  address, access constraints, costs and procedure. Link carrier confirmation
  and warehouse pick/pack, dispatch and return rehearsal evidence. Missing
  supplier measurements cannot be replaced by sample values.
- **VAT, OSS and documents:** qualified accountant's current destination-specific
  review and effective period for goods and shipping, VAT/OSS treatment,
  applicable rates and rounding, invoice and credit/refund document treatment,
  and evidence of the relevant operational setup. Record decisions and source
  references; do not assume uniform treatment across territories.
- **Legal and packaging:** qualified review for the actual goods and destination,
  including consumer/return terms, product, importer, textile and labelling
  obligations, and packaging obligations and any required registration or
  operational evidence. Identify any outstanding conditions and the applicable
  reviewed customer-facing content. Do not infer a legal exemption from a product
  option.
- **Acceptance and implementation:** the exact approved policy revision and
  validity window, linked publication/product revisions and permitted postcode
  scope; independent market review; evidence of quote and checkout tests for
  included, excluded and unknown destinations; owner and operational acceptance
  references. These are filled only after the underlying evidence is reviewed.

## Decision and release boundary

A ready decision needs current, mutually consistent evidence for every category
above and covers only the reviewed products, service and explicit postcode and
territory scope. Record exclusions and unresolved areas as disabled. A decision
in this register does not create a market policy or grant authority: a separate
private policy draft and independent review must be installed for the exact
scope, and missing, ambiguous, withdrawn or expired rules fail closed. Payment
must remain unavailable for an unready destination.

Country readiness is one input to release, not production activation. The
[release gates](OPERATIONS.md#release-gates) still require the exact release
revision, required checks and independent reviews, real market/provider/product/
legal acceptance, restore and rollback evidence, and owner approval for live
commerce. Operational acceptance separately calls for real mail, sandbox and
separately authorized live payment/refund, warehouse
and return rehearsal, correct VAT/invoice/credit documents and intended-market
coverage. A policy, source test, merge or deployment alone proves none of those
outcomes. Unready destinations continue to block the EU-wide launch intent.
