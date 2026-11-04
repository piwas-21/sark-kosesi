import type { components, paths } from "./generated";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2
    ? true
    : false;
type Assert<T extends true> = T;
type IsOptional<T, K extends keyof T> =
  object extends Pick<T, K> ? true : false;
type Catalogue = paths["/api/published/{locale}"]["get"];
type Image =
  paths["/api/published/{locale}/images/{sku}/{revision}/{edge}"]["get"];
type Revoke = paths["/api/guest/session"]["delete"];
type Write = paths["/api/guest/basket"]["put"];

export type ContractAssertions = [
  Assert<
    Equal<keyof Image["responses"][200]["content"], "image/jpeg" | "image/png">
  >,
  Assert<Equal<Image["responses"][200]["content"]["image/jpeg"], Blob | File>>,
  Assert<Equal<components["schemas"]["ImageBytes"], Blob | File>>,
  Assert<Equal<Revoke["responses"][204]["content"], undefined>>,
  Assert<Equal<IsOptional<Catalogue["parameters"], "path">, false>>,
  Assert<Equal<Catalogue["parameters"]["path"], { locale: string }>>,
  Assert<Equal<IsOptional<Catalogue["parameters"], "query">, true>>,
  Assert<
    Equal<NonNullable<Catalogue["parameters"]["query"]>, { after?: string }>
  >,
  Assert<
    Equal<
      "other" extends keyof NonNullable<Catalogue["parameters"]["query"]>
        ? true
        : false,
      false
    >
  >,
  Assert<Equal<IsOptional<Write, "requestBody">, false>>,
  Assert<
    Equal<
      Write["requestBody"]["content"]["application/json"],
      components["schemas"]["GuestBasketUpdate"]
    >
  >,
  Assert<
    Equal<
      components["schemas"]["Material"],
      | { fibres: components["schemas"]["Fibre"][]; kind: "cover" }
      | { density_g_per_m3: number; kind: "foam" }
    >
  >,
  Assert<
    Equal<
      components["schemas"]["PublishedCatalogue"]["next_after"],
      string | null | undefined
    >
  >,
];
