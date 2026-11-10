import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { plannerCopy } from "../src/components/room-planner/copy";
import { decimal } from "../src/components/room-planner/draft";
import { RoomPlanner } from "../src/components/room-planner/planner";

const copy = plannerCopy.en;
const group = (name: string) => within(screen.getByRole("group", { name }));
const room = () => within(screen.getByRole("region", { name: copy.room }));
const change = (input: HTMLElement, value: string) =>
  fireEvent.change(input, { target: { value } });
const click = (name: string) =>
  fireEvent.click(screen.getByRole("button", { name }));
const result = () =>
  within(screen.getByRole("complementary", { name: copy.drawing })).getByRole(
    "status",
  );

function fillRoom() {
  change(room().getByLabelText("Width (cm)"), "400");
  change(room().getByLabelText("Depth (cm)"), "300");
}

function fillZone(label: string, width: string, depth: string, left = "0") {
  const zone = group(label);
  change(zone.getByLabelText("Width (cm)"), width);
  change(zone.getByLabelText("Depth (cm)"), depth);
  change(zone.getByLabelText("From the left (cm)"), left);
}

describe("local room planner", () => {
  it("normalizes decimal keyboard input without treating grouping as a number", () => {
    expect(decimal(" ٠٠١٢٫٣ ")).toBe("12.3");
    expect(decimal("۰,۵")).toBe("0.5");
    expect(decimal(".25")).toBe("0.25");
    expect(decimal("1,234.5")).toBe("1.234.5");
  });

  it("shows field errors, locks the units and clears the draft on request", () => {
    render(<RoomPlanner copy={copy} />);
    click(copy.check);
    expect(result()).toHaveTextContent(copy.invalid);
    expect(room().getByLabelText("Width (cm)")).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    change(room().getByLabelText("Width (cm)"), "10001");
    click(copy.check);
    expect(room().getByLabelText("Width (cm)")).toHaveAccessibleDescription(
      expect.stringContaining("at most 100 m"),
    );
    change(room().getByLabelText("Width (cm)"), "400");
    expect(room().getByLabelText("Width (cm)")).toHaveAttribute(
      "aria-invalid",
      "false",
    );
    expect(room().getByLabelText("Depth (cm)")).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    expect(group("Piece 1").getByLabelText("Width (cm)")).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    change(room().getByLabelText("Depth (cm)"), "300");
    const unit = screen.getByLabelText(copy.unit);
    expect(unit).toBeDisabled();
    click(copy.clear);
    expect(room().getByLabelText("Width (cm)")).toHaveValue("");
    expect(unit).toBeEnabled();
    change(unit, "in");
    expect(room().getByLabelText("Width (in)")).toBeInTheDocument();
  });

  it("reports conflicts, unknown clearance, then a fit after rotation", () => {
    render(<RoomPlanner copy={copy} />);
    fillRoom();
    fillZone("Piece 1", "150", "70", "280");
    click(copy.check);
    expect(result()).toHaveTextContent("Piece 1 extends beyond the room.");
    change(group("Piece 1").getByLabelText(copy.rotation), "90");
    click(copy.check);
    expect(result()).toHaveTextContent(copy.unknownDoors);
    change(screen.getByLabelText(copy.doorStatus), "none");
    click(copy.check);
    expect(result()).toHaveTextContent(copy.fit);
  });

  it("identifies blocked clearance and overlap, then reassesses after removal", () => {
    render(<RoomPlanner copy={copy} />);
    fillRoom();
    fillZone("Piece 1", "150", "70");
    click(copy.addPiece);
    fillZone("Piece 2", "20", "20");
    change(screen.getByLabelText(copy.doorStatus), "measured");
    fillZone("Clearance zone 1", "20", "20");
    click(copy.addDoor);
    fillZone("Clearance zone 2", "20", "20", "300");
    click(copy.check);
    expect(result()).toHaveTextContent("Piece 2 overlaps Piece 1.");
    expect(result()).toHaveTextContent("Piece 1 blocks Clearance zone 1.");
    click("Remove Clearance zone 1");
    click("Remove Piece 2");
    click(copy.check);
    expect(result()).toHaveTextContent(copy.fit);
    click("Remove Piece 1");
    click(copy.check);
    expect(result()).toHaveTextContent(copy.noPieces);
    change(room().getByLabelText("Measurement allowance per side (cm)"), "150");
    click(copy.check);
    expect(result()).toHaveTextContent(copy.toleranceTooLarge);
  });
});
