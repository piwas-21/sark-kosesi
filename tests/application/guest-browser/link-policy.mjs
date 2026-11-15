// Only the private loopback proxy may supply an omitted same-origin GET Origin.
export function allowsBrowserHistory({ method, url, site, origin }, expected) {
  return (
    method === "GET" &&
    url === "/api/test/customer/linked-orders" &&
    site === "same-origin" &&
    (origin === undefined || origin === expected)
  );
}
