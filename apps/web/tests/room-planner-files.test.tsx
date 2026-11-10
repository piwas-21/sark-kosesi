import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { plannerCopy } from "../src/components/room-planner/copy";
import { RoomPlanner } from "../src/components/room-planner/planner";

const copy = plannerCopy.en;
const fileCopy = copy.files;
const plan =
  '{"format":"piwas-room-plan","version":1,"unit":"mm","room":{"width":"4000","depth":"3000","tolerance":"5"},"items":[{"width":"1000","depth":"500","left":"10","top":"20","rotation":90},{"width":"200","depth":"100","left":"1200","top":"0","rotation":270}],"doorMode":"measured","doors":[{"width":"300","depth":"200","left":"2000","top":"0"}]}';

function buffer(text: string): ArrayBuffer {
  const bytes = new TextEncoder().encode(text);
  const result = new ArrayBuffer(bytes.length);
  new Uint8Array(result).set(bytes);
  return result;
}
function file(text: string, read = vi.fn(async () => buffer(text))): File {
  const value = new File([text], "plan.json", { type: "application/json" });
  Object.defineProperty(value, "arrayBuffer", { value: read });
  return value;
}
function room() {
  return within(screen.getByRole("region", { name: copy.room }));
}
function piece(index: number) {
  return within(screen.getByRole("group", { name: `${copy.piece} ${index}` }));
}
function files() {
  return within(screen.getByRole("region", { name: fileCopy.heading }));
}
function select(value: File) {
  const input = screen.getByLabelText(fileCopy.open, { selector: "input" });
  fireEvent.change(input, {
    target: {
      files: {
        0: value,
        length: 1,
        item: (index: number) => (index === 0 ? value : null),
      },
    },
  });
}
function originalDraft() {
  fireEvent.change(room().getByLabelText("Width (cm)"), {
    target: { value: "400" },
  });
  fireEvent.change(room().getByLabelText("Depth (cm)"), {
    target: { value: "300" },
  });
  fireEvent.change(piece(1).getByLabelText("Width (cm)"), {
    target: { value: "150" },
  });
  fireEvent.change(piece(1).getByLabelText("Depth (cm)"), {
    target: { value: "70" },
  });
  fireEvent.change(piece(1).getByLabelText("From the left (cm)"), {
    target: { value: "280" },
  });
  fireEvent.click(screen.getByRole("button", { name: copy.check }));
  expect(
    screen.getByText("Piece 1 extends beyond the room."),
  ).toBeInTheDocument();
}
function expectOriginal() {
  expect(room().getByLabelText("Width (cm)")).toHaveValue("400");
  expect(piece(1).getByLabelText("From the left (cm)")).toHaveValue("280");
  expect(
    screen.getByText("Piece 1 extends beyond the room."),
  ).toBeInTheDocument();
}

afterEach(() => vi.unstubAllGlobals());

describe("local room planner files", () => {
  it("stages a valid file and replaces the plan only after explicit confirmation", async () => {
    const fetch = vi.fn();
    vi.stubGlobal("fetch", fetch);
    const storage = vi.spyOn(Storage.prototype, "setItem");
    const storageRead = vi.spyOn(Storage.prototype, "getItem");
    const storageRemove = vi.spyOn(Storage.prototype, "removeItem");
    render(<RoomPlanner copy={copy} />);
    originalDraft();
    select(file(plan));
    await screen.findByRole("button", { name: fileCopy.apply });
    expect(files().getByRole("status")).toHaveTextContent(fileCopy.replace);
    expectOriginal();
    fireEvent.click(screen.getByRole("button", { name: fileCopy.apply }));
    expect(room().getByLabelText("Width (mm)")).toHaveValue("4000");
    expect(room().getByLabelText("Depth (mm)")).toHaveValue("3000");
    expect(
      room().getByLabelText("Measurement allowance per side (mm)"),
    ).toHaveValue("5");
    expect(piece(1).getByLabelText(copy.rotation)).toHaveValue("90");
    expect(piece(2).getByLabelText(copy.rotation)).toHaveValue("270");
    expect(screen.getByLabelText(copy.doorStatus)).toHaveValue("measured");
    expect(
      within(
        screen.getByRole("group", { name: `${copy.door} 1` }),
      ).getByLabelText("Width (mm)"),
    ).toHaveValue("300");
    expect(screen.getByText(copy.pending)).toBeInTheDocument();
    expect(
      screen.queryByText("Piece 1 extends beyond the room."),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: fileCopy.open })).toHaveFocus();
    expect(fetch).not.toHaveBeenCalled();
    expect(storage).not.toHaveBeenCalled();
    expect(storageRead).not.toHaveBeenCalled();
    expect(storageRemove).not.toHaveBeenCalled();
  });

  it("cancels a staged replacement without changing the draft or result", async () => {
    render(<RoomPlanner copy={copy} />);
    originalDraft();
    select(file(plan));
    await screen.findByRole("button", { name: fileCopy.cancel });
    fireEvent.click(screen.getByRole("button", { name: fileCopy.cancel }));
    expectOriginal();
    expect(
      screen.queryByRole("button", { name: fileCopy.apply }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: fileCopy.open })).toHaveFocus();
  });

  it("announces a failed download without hiding a staged replacement", async () => {
    render(<RoomPlanner copy={copy} />);
    fireEvent.change(room().getByLabelText("Width (cm)"), {
      target: { value: "not a measurement" },
    });
    select(file(plan));
    await screen.findByRole("button", { name: fileCopy.apply });
    fireEvent.click(screen.getByRole("button", { name: fileCopy.download }));
    expect(files().getByRole("status")).toHaveTextContent(fileCopy.cannotSave);
    expect(files().getByRole("status")).toHaveTextContent(fileCopy.replace);
    expect(files().getByRole("button", { name: fileCopy.apply })).toBeEnabled();
    expect(
      files().getByRole("button", { name: fileCopy.cancel }),
    ).toBeEnabled();
    expect(room().getByLabelText("Width (cm)")).toHaveValue(
      "not a measurement",
    );
  });

  it("clears stale field errors when a validated plan is applied", async () => {
    render(<RoomPlanner copy={copy} />);
    fireEvent.click(screen.getByRole("button", { name: copy.check }));
    expect(room().getByLabelText("Width (cm)")).toHaveAttribute(
      "aria-invalid",
      "true",
    );
    select(file(plan));
    fireEvent.click(
      await screen.findByRole("button", { name: fileCopy.apply }),
    );
    expect(room().getByLabelText("Width (mm)")).toHaveValue("4000");
    expect(room().getByLabelText("Width (mm)")).not.toHaveAttribute(
      "aria-invalid",
      "true",
    );
    expect(screen.getByText(copy.pending)).toBeInTheDocument();
  });

  it("keeps an imported measured plan without zones uncertain while explicit none can fit", async () => {
    const withoutDoors = plan
      .replace('"top":"0","rotation":270', '"top":"20","rotation":270')
      .replace(/"doors":\[.*\]\}$/, '"doors":[]}');
    render(<RoomPlanner copy={copy} />);
    select(file(withoutDoors));
    fireEvent.click(
      await screen.findByRole("button", { name: fileCopy.apply }),
    );
    expect(screen.getByLabelText(copy.doorStatus)).toHaveValue("measured");
    fireEvent.click(screen.getByRole("button", { name: copy.check }));
    const result = () =>
      within(
        screen.getByRole("complementary", { name: copy.drawing }),
      ).getByRole("status");
    expect(result()).toHaveTextContent(copy.unknownDoors);
    expect(result()).not.toHaveTextContent(copy.fit);

    select(
      file(withoutDoors.replace('"doorMode":"measured"', '"doorMode":"none"')),
    );
    fireEvent.click(
      await screen.findByRole("button", { name: fileCopy.apply }),
    );
    expect(screen.getByLabelText(copy.doorStatus)).toHaveValue("none");
    fireEvent.click(screen.getByRole("button", { name: copy.check }));
    expect(result()).toHaveTextContent(copy.fit);
  });

  it("rejects invalid, malformed UTF-8 and oversized files without changing the plan", async () => {
    render(<RoomPlanner copy={copy} />);
    originalDraft();
    select(file('{"format":"piwas-room-plan","version":2}'));
    await waitFor(() =>
      expect(files().getByRole("status")).toHaveTextContent(fileCopy.invalid),
    );
    expectOriginal();
    const readBadUtf8 = vi.fn(async () => new Uint8Array([0xc3, 0x28]).buffer);
    const badUtf8 = file("", readBadUtf8);
    select(badUtf8);
    await waitFor(() => expect(readBadUtf8).toHaveBeenCalledOnce());
    await waitFor(() =>
      expect(files().getByRole("status")).toHaveTextContent(fileCopy.invalid),
    );
    expectOriginal();
    const read = vi.fn(async () => buffer(plan));
    const oversized = new File(["x".repeat(16_385)], "large.json");
    Object.defineProperty(oversized, "arrayBuffer", { value: read });
    select(oversized);
    await waitFor(() =>
      expect(files().getByRole("status")).toHaveTextContent(fileCopy.tooLarge),
    );
    expect(read).not.toHaveBeenCalled();
    expectOriginal();
    expect(
      screen.queryByRole("button", { name: fileCopy.apply }),
    ).not.toBeInTheDocument();
  });

  it("does not let an earlier slow file selection replace a newer one", async () => {
    render(<RoomPlanner copy={copy} />);
    originalDraft();
    let release: (value: ArrayBuffer) => void = () => {
      throw new Error("missing resolver");
    };
    const slow = new Promise<ArrayBuffer>((resolve) => {
      release = resolve;
    });
    select(
      file(
        plan,
        vi.fn(() => slow),
      ),
    );
    select(file(plan.replace('"width":"4000"', '"width":"5000"')));
    await screen.findByRole("button", { name: fileCopy.apply });
    await act(async () => {
      release(buffer(plan));
      await slow;
    });
    fireEvent.click(screen.getByRole("button", { name: fileCopy.apply }));
    expect(room().getByLabelText("Width (mm)")).toHaveValue("5000");
  });

  it("downloads a local JSON Blob and revokes its object URL", async () => {
    const create = vi.fn<(blob: Blob | MediaSource) => string>(
      () => "blob:synthetic-plan",
    );
    const revoke = vi.fn();
    const OriginalURL = URL;
    vi.stubGlobal(
      "URL",
      class extends OriginalURL {
        static createObjectURL = create;
        static revokeObjectURL = revoke;
      },
    );
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, "click")
      .mockImplementation(() => {});
    render(<RoomPlanner copy={copy} />);
    originalDraft();
    fireEvent.click(screen.getByRole("button", { name: fileCopy.download }));
    expect(create).toHaveBeenCalledOnce();
    expect(click).toHaveBeenCalledOnce();
    expect(revoke).not.toHaveBeenCalled();
    await waitFor(
      () => expect(revoke).toHaveBeenCalledWith("blob:synthetic-plan"),
      { timeout: 2500 },
    );
    const blob: unknown = create.mock.calls[0]?.[0];
    expect(blob).toBeInstanceOf(Blob);
    if (!(blob instanceof Blob)) throw new Error("expected local Blob");
    const text = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () =>
        typeof reader.result === "string"
          ? resolve(reader.result)
          : reject(new Error("expected text"));
      reader.onerror = () => reject(new Error("cannot read local Blob"));
      reader.readAsText(blob);
    });
    expect(JSON.parse(text)).toMatchObject({
      format: "piwas-room-plan",
      version: 1,
      unit: "cm",
      room: { width: "400", depth: "300" },
    });
    expectOriginal();
  });
});
