import { render, screen } from "@testing-library/react";
import { it, expect } from "vitest";
import { Button, Field, Status, CourtyardMark } from "../src/primitives";
it("renders labelled input, error, disabled action and live status", () => {
  render(
    <>
      <Button disabled>Unavailable</Button>
      <Field id="width" label="Width" error="Enter a positive width" />
      <Status busy>Loading</Status>
      <CourtyardMark />
    </>,
  );
  expect(screen.getByRole("button")).toBeDisabled();
  expect(screen.getByLabelText("Width")).toHaveAccessibleDescription(
    "Enter a positive width",
  );
  expect(screen.getByRole("alert")).toHaveTextContent("positive");
  expect(screen.getByRole("status")).toHaveAttribute("aria-busy", "true");
});
it("renders valid states and custom classes", () => {
  render(
    <>
      <Button className="extra">Continue</Button>
      <Field id="name" label="Name" />
      <Status>Ready</Status>
    </>,
  );
  expect(screen.getByRole("button")).toHaveClass("extra");
  expect(screen.getByLabelText("Name")).toHaveAttribute(
    "aria-invalid",
    "false",
  );
  expect(screen.getByRole("status")).toHaveAttribute("aria-busy", "false");
});
