import { Button, Field, Status } from "@piwas/design";
export default function Gallery() {
  return (
    <div lang="en">
      <h1>Component study</h1>
      <section aria-labelledby="actions">
        <h2 id="actions">Actions and focus</h2>
        <Button type="button">Preview action</Button>{" "}
        <Button type="button" disabled>
          Unavailable
        </Button>
        <p>
          Use Tab to inspect keyboard focus. This gallery does not save changes.
        </p>
      </section>
      <section aria-labelledby="fields">
        <h2 id="fields">Fields and errors</h2>
        <Field
          id="room"
          label="Room width (cm)"
          type="number"
          defaultValue="0"
          error="Enter a width greater than zero."
        />
        <Field id="disabled" label="Unavailable field" disabled />
      </section>
      <section aria-labelledby="states">
        <h2 id="states">Status and empty states</h2>
        <Status busy>Loading demonstration catalogue…</Status>
        <Status>No items in this preview.</Status>
      </section>
    </div>
  );
}
