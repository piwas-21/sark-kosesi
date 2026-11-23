export const reviewContent = {
  storeName: "Testcollectie",
  sellerName: "Testverkoper",
  kvk: "01234567",
  vatId: "NL012345678B01",
  address: "Teststraat 1\n0000 AA Testplaats",
  email: "contact@example.test",
  phone: "+31123456789",
  policies: {
    delivery: "Testtekst voor bezorging.",
    returns: "Testtekst voor retourneren. <script>returnPolicy</script>",
    privacy: "Testtekst voor privacy.",
    payment: "Testtekst voor betalen.",
    terms: "Testtekst voor verkoopvoorwaarden.",
  },
};

export const reviewEnvironment = {
  APP_ENV: "staging",
  PREVIEW_TOKEN: "s".repeat(32),
  PREVIEW_REVISION: "b".repeat(40),
  API_URL: "http://127.0.0.1:45671",
  CANONICAL_ORIGIN: "https://review.example.test",
  STAGING_PUBLIC_STOREFRONT: "true",
  STAGING_ADMIN_TOKEN: "p".repeat(32),
  STAGING_STORE_REVIEW_CONTENT: JSON.stringify(reviewContent),
};
