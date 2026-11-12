// Generated from Rust OpenAPI and Hey API types. Do not edit.
import type * as Model from "./models.gen";

export interface components {
  schemas: {
    ApiError: Model.ApiError;
    Availability: Model.Availability;
    CallbackForm: Model.CallbackForm;
    Catalogue: Model.Catalogue;
    Component: Model.Component;
    Fibre: Model.Fibre;
    GuestBasket: Model.GuestBasket;
    GuestBasketReceipt: Model.GuestBasketReceipt;
    GuestBasketUpdate: Model.GuestBasketUpdate;
    GuestIssueRequest: Model.GuestIssueRequest;
    GuestLine: Model.GuestLine;
    ImageBytes: Model.ImageBytes;
    Material: Model.Material;
    Measurements: Model.Measurements;
    Parcel: Model.Parcel;
    Product: Model.Product;
    ProductCopy: Model.ProductCopy;
    ProductImage: Model.ProductImage;
    PublishedCatalogue: Model.PublishedCatalogue;
    PublishedProduct: Model.PublishedProduct;
    QuoteAmounts: Model.QuoteAmounts;
    QuoteRequestBody: Model.QuoteRequestBody;
    QuoteResponse: Model.QuoteResponse;
    QuoteView: Model.QuoteView;
    QuotedLine: Model.QuotedLine;
    RedeemRequest: Model.RedeemRequest;
  };
}

export interface paths {
  "/api/callbacks/mollie": {
    post: operations["intake"];
  };
  "/api/catalogue/{locale}": {
    get: operations["get_catalogue"];
  };
  "/api/guest/basket": {
    get: operations["read"];
    put: operations["write"];
  };
  "/api/guest/quotes/{id}": {
    get: operations["read_guest_quote"];
  };
  "/api/guest/session": {
    post: operations["issue"];
    delete: operations["revoke"];
  };
  "/api/guest/{locale}/quotes": {
    post: operations["issue_guest_quote"];
  };
  "/api/published/{locale}": {
    get: operations["page"];
  };
  "/api/published/{locale}/images/{sku}/{revision}/{edge}": {
    get: operations["image"];
  };
  "/api/published/{locale}/products/{sku}": {
    get: operations["product"];
  };
  "/api/test/verification/redeem": {
    post: operations["redeem_test_verification"];
  };
}

export interface operations {
  "intake": {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/x-www-form-urlencoded": Model.CallbackForm; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content?: never };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "get_catalogue": {
    parameters: {
      query?: never;
      header?: never;
      path: NonNullable<Model.GetCatalogueData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.Catalogue; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "read": {
    parameters: {
      query?: never;
      header?: never;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.GuestBasket; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "write": {
    parameters: {
      query?: never;
      header: NonNullable<Model.WriteData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.GuestBasketUpdate; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.GuestBasketReceipt; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      429: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "read_guest_quote": {
    parameters: {
      query?: never;
      header?: never;
      path: NonNullable<Model.ReadGuestQuoteData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.QuoteView; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      410: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "issue": {
    parameters: {
      query?: never;
      header: NonNullable<Model.IssueData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.GuestIssueRequest; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.GuestBasket; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      429: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "revoke": {
    parameters: {
      query?: never;
      header: NonNullable<Model.RevokeData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      204: { headers: { [name: string]: unknown }; content?: never };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "issue_guest_quote": {
    parameters: {
      query?: never;
      header: NonNullable<Model.IssueGuestQuoteData["headers"]>;
      path: NonNullable<Model.IssueGuestQuoteData["path"]>;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.QuoteRequestBody; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.QuoteResponse; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      410: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      429: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "page": {
    parameters: {
      query?: NonNullable<Model.PageData["query"]>;
      header?: never;
      path: NonNullable<Model.PageData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.PublishedCatalogue; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "image": {
    parameters: {
      query?: never;
      header?: never;
      path: NonNullable<Model.ImageData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "image/jpeg": Model.ImageBytes; "image/png": Model.ImageBytes; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "product": {
    parameters: {
      query?: never;
      header?: never;
      path: NonNullable<Model.ProductData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.PublishedProduct; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "redeem_test_verification": {
    parameters: {
      query?: never;
      header: NonNullable<Model.RedeemTestVerificationData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.RedeemRequest; } };
    responses: {
      204: { headers: { [name: string]: unknown }; content?: never };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
}
