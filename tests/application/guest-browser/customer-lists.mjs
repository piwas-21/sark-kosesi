import assert from "node:assert/strict";
import { isDeepStrictEqual } from "node:util";

export const customerListFlags = [
  "TEST_FAVOURITES_TRANSPORT",
  "TEST_FAVOURITES_MUTATION_TRANSPORT",
  "TEST_RESTOCK_TRANSPORT",
  "TEST_RESTOCK_MUTATION_TRANSPORT",
];

const sku = "SET-FILLED";
const favourites = { name: "favourites", field: "favourite" };
const restock = { name: "restock", field: "subscribed" };
const lists = [favourites, restock].map((list) => {
  const path = `/api/test/customer/${list.name}/nl`;
  return { ...list, path, itemPath: `${path}/${sku}` };
});

export const customerListPaths = lists.map((list) => list.path);

export const customerListRoutes = [
  ["GET", /^\/api\/test\/customer\/(?:favourites|restock)\/nl$/],
  ["PUT", /^\/api\/test\/customer\/(?:favourites|restock)\/nl\/SET-FILLED$/],
];

// Supply an omitted read Origin only for a same-origin fetch to the exact test lists.
export function allowsBrowserCustomerList(
  { method, url, site, origin },
  expected,
) {
  return (
    method === "GET" &&
    lists.some((list) => list.path === url) &&
    site === "same-origin" &&
    (origin === undefined || origin === expected)
  );
}

function expectResponse(response, status, label, body) {
  assert.equal(response.status, status, `${label} status`);
  assert.equal(response.cache, "private, no-store", `${label} cache policy`);
  if (body !== undefined)
    assert(isDeepStrictEqual(response.body, body), `${label} closed response`);
}

function listResponse(revision, published = false) {
  return {
    revision,
    items: published ? [{ sku, publication_revision: 1 }] : [],
  };
}

function update(call, list, wanted, revision, csrf) {
  return call(
    list.itemPath,
    "PUT",
    {
      [list.field]: wanted,
      expected_revision: revision,
    },
    csrf,
  );
}

export async function checkAnonymousCustomerLists(call) {
  for (const list of lists) {
    expectResponse(await call(list.path, "GET"), 401, `anonymous ${list.name}`);
    expectResponse(
      await update(call, list, true, "0", undefined),
      401,
      `anonymous ${list.name} mutation`,
    );
  }
}

export async function checkCustomerLists({
  owner,
  other,
  ownerCsrf,
  otherCsrf,
  withdraw,
}) {
  assert(
    ownerCsrf !== otherCsrf,
    "fixture sessions must have different proofs",
  );
  for (const list of lists) {
    expectResponse(
      await owner(list.path, "GET"),
      200,
      `empty owner ${list.name}`,
      listResponse("0"),
    );
    expectResponse(
      await other(list.path, "GET"),
      200,
      `empty second-account ${list.name}`,
      listResponse("0"),
    );
    expectResponse(
      await update(owner, list, true, "0", otherCsrf),
      401,
      `foreign proof on owner ${list.name}`,
    );
    expectResponse(
      await update(other, list, true, "0", ownerCsrf),
      401,
      `foreign proof on second-account ${list.name}`,
    );

    // Exact repeats can report the same successful revision without a duplicate membership.
    const repeated = await Promise.all([
      update(owner, list, true, "0", ownerCsrf),
      update(owner, list, true, "0", ownerCsrf),
    ]);
    for (const response of repeated)
      expectResponse(response, 200, `confirmed ${list.name}`, {
        revision: "1",
      });
    expectResponse(
      await owner(list.path, "GET"),
      200,
      `published owner ${list.name}`,
      listResponse("1", true),
    );
    expectResponse(
      await other(list.path, "GET"),
      200,
      `isolated second-account ${list.name}`,
      listResponse("0"),
    );
    expectResponse(
      await update(owner, list, false, "0", ownerCsrf),
      409,
      `stale ${list.name} change`,
    );
    if (list.name === favourites.name) {
      expectResponse(
        await owner(lists[1].path, "GET"),
        200,
        "favourite did not subscribe",
        listResponse("0"),
      );
    } else {
      expectResponse(
        await owner(lists[0].path, "GET"),
        200,
        "subscription left favourites unchanged",
        listResponse("1", true),
      );
    }
  }

  // The trusted fixture withdraws its own publication between browser requests.
  await withdraw();
  for (const list of lists) {
    expectResponse(
      await owner(list.path, "GET"),
      200,
      `hidden owner ${list.name}`,
      listResponse("1"),
    );
    expectResponse(
      await update(owner, list, false, "1", ownerCsrf),
      200,
      `remove hidden ${list.name}`,
      { revision: "2" },
    );
    expectResponse(
      await update(owner, list, true, "2", ownerCsrf),
      503,
      `withdrawn ${list.name} cannot restore`,
    );
    expectResponse(
      await owner(list.path, "GET"),
      200,
      `removed owner ${list.name}`,
      listResponse("2"),
    );
    expectResponse(
      await other(list.path, "GET"),
      200,
      `unchanged second-account ${list.name}`,
      listResponse("0"),
    );
  }
}

// The logout journey reuses the cleared owned buckets to prove the new proof
// can authorize a protected action without changing either membership.
export async function checkNewCustomerListProof(call, proof) {
  for (const list of lists) {
    expectResponse(
      await update(call, list, false, "2", proof),
      200,
      `fresh session ${list.name} authorization`,
      { revision: "2" },
    );
  }
}
