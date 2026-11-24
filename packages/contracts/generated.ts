// Generated from Rust OpenAPI and Hey API types. Do not edit.
import type * as Model from "./models.gen";

export interface components {
  schemas: {
    AddressInput: Model.AddressInput;
    ApiError: Model.ApiError;
    Availability: Model.Availability;
    BuyerInput: Model.BuyerInput;
    CallbackForm: Model.CallbackForm;
    CancellationRequest: Model.CancellationRequest;
    CancellationResponse: Model.CancellationResponse;
    Catalogue: Model.Catalogue;
    Component: Model.Component;
    Fibre: Model.Fibre;
    GrossPaymentProgress: Model.GrossPaymentProgress;
    GuestBasket: Model.GuestBasket;
    GuestBasketReceipt: Model.GuestBasketReceipt;
    GuestBasketUpdate: Model.GuestBasketUpdate;
    GuestIssueRequest: Model.GuestIssueRequest;
    GuestLine: Model.GuestLine;
    GuestOrderProgress: Model.GuestOrderProgress;
    ImageBytes: Model.ImageBytes;
    LoginRequest: Model.LoginRequest;
    Material: Model.Material;
    Measurements: Model.Measurements;
    OrderLifecycle: Model.OrderLifecycle;
    Parcel: Model.Parcel;
    PendingCancellationLookup: Model.PendingCancellationLookup;
    PendingCancellationReceipt: Model.PendingCancellationReceipt;
    PendingCancellationRecord: Model.PendingCancellationRecord;
    PendingCancellationRequest: Model.PendingCancellationRequest;
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
    TestFavouriteItem: Model.TestFavouriteItem;
    TestFavouriteList: Model.TestFavouriteList;
    TestFavouriteMutation: Model.TestFavouriteMutation;
    TestFavouriteReceipt: Model.TestFavouriteReceipt;
    TestGuestLinkRedemption: Model.TestGuestLinkRedemption;
    TestLinkedOrder: Model.TestLinkedOrder;
    TestLinkedOrderPage: Model.TestLinkedOrderPage;
    TestOrderAmount: Model.TestOrderAmount;
    TestOrderInput: Model.TestOrderInput;
    TestOrderLine: Model.TestOrderLine;
    TestOrderReceipt: Model.TestOrderReceipt;
    TestRestockItem: Model.TestRestockItem;
    TestRestockList: Model.TestRestockList;
    TestRestockMutation: Model.TestRestockMutation;
    TestRestockReceipt: Model.TestRestockReceipt;
    TestSessionProof: Model.TestSessionProof;
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
  "/api/test/customer/favourites/{locale}": {
    get: operations["test_customer_favourites"];
  };
  "/api/test/customer/favourites/{locale}/{sku}": {
    put: operations["test_customer_favourites_set"];
  };
  "/api/test/customer/guest-order-link/redeem": {
    post: operations["test_customer_guest_order_link_redeem"];
  };
  "/api/test/customer/linked-orders": {
    get: operations["test_customer_linked_orders"];
  };
  "/api/test/customer/login": {
    post: operations["test_customer_login"];
  };
  "/api/test/customer/login-with-csrf": {
    post: operations["test_customer_login_with_csrf"];
  };
  "/api/test/customer/restock/{locale}": {
    get: operations["test_customer_restock"];
  };
  "/api/test/customer/restock/{locale}/{sku}": {
    put: operations["test_customer_restock_set"];
  };
  "/api/test/customer/session": {
    delete: operations["test_customer_logout"];
  };
  "/api/test/guest/orders": {
    post: operations["test_guest_order_create"];
  };
  "/api/test/guest/orders/{id}/cancellation": {
    post: operations["test_guest_cancel_order"];
  };
  "/api/test/guest/orders/{id}/pending-cancellation": {
    get: operations["test_guest_pending_cancellation_status"];
    post: operations["test_guest_request_pending_cancellation"];
  };
  "/api/test/guest/orders/{id}/progress": {
    get: operations["test_guest_order_progress"];
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
  "test_customer_favourites": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerFavouritesData["headers"]>;
      path: NonNullable<Model.TestCustomerFavouritesData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestFavouriteList; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_favourites_set": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerFavouritesSetData["headers"]>;
      path: NonNullable<Model.TestCustomerFavouritesSetData["path"]>;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.TestFavouriteMutation; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestFavouriteReceipt; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_guest_order_link_redeem": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerGuestOrderLinkRedeemData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.TestGuestLinkRedemption; } };
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
  "test_customer_linked_orders": {
    parameters: {
      query?: NonNullable<Model.TestCustomerLinkedOrdersData["query"]>;
      header: NonNullable<Model.TestCustomerLinkedOrdersData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestLinkedOrderPage; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_login": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerLoginData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.LoginRequest; } };
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
  "test_customer_login_with_csrf": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerLoginWithCsrfData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.LoginRequest; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestSessionProof; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_restock": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerRestockData["headers"]>;
      path: NonNullable<Model.TestCustomerRestockData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestRestockList; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_restock_set": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerRestockSetData["headers"]>;
      path: NonNullable<Model.TestCustomerRestockSetData["path"]>;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.TestRestockMutation; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestRestockReceipt; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_customer_logout": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestCustomerLogoutData["headers"]>;
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
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_guest_order_create": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestGuestOrderCreateData["headers"]>;
      path?: never;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.TestOrderInput; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.TestOrderReceipt; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      410: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      429: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_guest_cancel_order": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestGuestCancelOrderData["headers"]>;
      path: NonNullable<Model.TestGuestCancelOrderData["path"]>;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.CancellationRequest; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.CancellationResponse; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_guest_pending_cancellation_status": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestGuestPendingCancellationStatusData["headers"]>;
      path: NonNullable<Model.TestGuestPendingCancellationStatusData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.PendingCancellationLookup; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_guest_request_pending_cancellation": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestGuestRequestPendingCancellationData["headers"]>;
      path: NonNullable<Model.TestGuestRequestPendingCancellationData["path"]>;
      cookie?: never;
    };
    requestBody: { content: { "application/json": Model.PendingCancellationRequest; } };
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.PendingCancellationReceipt; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      404: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      409: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      413: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      415: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      503: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      504: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
    };
  };
  "test_guest_order_progress": {
    parameters: {
      query?: never;
      header: NonNullable<Model.TestGuestOrderProgressData["headers"]>;
      path: NonNullable<Model.TestGuestOrderProgressData["path"]>;
      cookie?: never;
    };
    requestBody?: never;
    responses: {
      200: { headers: { [name: string]: unknown }; content: { "application/json": Model.GuestOrderProgress; } };
      400: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      401: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
      403: { headers: { [name: string]: unknown }; content: { "application/json": Model.ApiError; } };
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
