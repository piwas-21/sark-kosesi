"use client";

import { useEffect, useId, useRef, useState, type ChangeEvent } from "react";
import {
  decodeRoomDraft,
  encodeRoomDraft,
  MAX_ROOM_DRAFT_BYTES,
  type RoomDraft,
} from "@piwas/room-draft";
import type { PlannerCopy } from "./copy";
import styles from "./draft-files.module.css";

export function DraftFiles({
  copy,
  draft,
  onApply,
}: {
  copy: PlannerCopy["files"];
  draft: RoomDraft;
  onApply: (draft: RoomDraft) => void;
}) {
  const heading = useId();
  const input = useRef<HTMLInputElement>(null);
  const openButton = useRef<HTMLButtonElement>(null);
  const request = useRef(0);
  const [pending, setPending] = useState<RoomDraft | null>(null);
  const [status, setStatus] = useState("");

  useEffect(
    () => () => {
      request.current++;
    },
    [],
  );

  function download() {
    const encoded = encodeRoomDraft(draft);
    if (!encoded.ok) {
      setStatus(copy.cannotSave);
      return;
    }
    let url: string | undefined;
    let anchor: HTMLAnchorElement | undefined;
    try {
      url = URL.createObjectURL(
        new Blob([encoded.value], { type: "application/json" }),
      );
      anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = "room-plan.json";
      document.body.append(anchor);
      anchor.click();
      setStatus("");
    } catch {
      setStatus(copy.cannotSave);
    } finally {
      anchor?.remove();
      if (url !== undefined) {
        const completedUrl = url;
        // Let the browser resolve the download before releasing the file.
        window.setTimeout(() => URL.revokeObjectURL(completedUrl), 1000);
      }
    }
  }

  function open() {
    request.current++;
    setPending(null);
    setStatus("");
    if (input.current) {
      input.current.value = "";
      input.current.click();
    }
  }

  async function select(event: ChangeEvent<HTMLInputElement>) {
    event.stopPropagation();
    const selected = event.currentTarget.files?.item(0);
    const current = ++request.current;
    setPending(null);
    setStatus("");
    if (!selected) return;
    if (selected.size > MAX_ROOM_DRAFT_BYTES) {
      setStatus(copy.tooLarge);
      return;
    }
    try {
      const bytes = await selected.arrayBuffer();
      if (current !== request.current) return;
      if (bytes.byteLength > MAX_ROOM_DRAFT_BYTES) {
        setStatus(copy.tooLarge);
        return;
      }
      const json = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
      const decoded = decodeRoomDraft(json);
      if (!decoded.ok) {
        setStatus(decoded.error === "too-large" ? copy.tooLarge : copy.invalid);
        return;
      }
      setPending(decoded.value);
    } catch {
      if (current === request.current) setStatus(copy.invalid);
    }
  }

  function cancel() {
    request.current++;
    setPending(null);
    setStatus("");
    openButton.current?.focus();
  }

  function apply() {
    if (!pending) return;
    request.current++;
    onApply(pending);
    setPending(null);
    setStatus(copy.imported);
    openButton.current?.focus();
  }

  return (
    <section className={styles.section} aria-labelledby={heading}>
      <h2 id={heading}>{copy.heading}</h2>
      <p className={styles.note}>{copy.privacy}</p>
      <div className={styles.actions}>
        <button type="button" className={styles.action} onClick={download}>
          {copy.download}
        </button>
        <button
          ref={openButton}
          type="button"
          className={styles.action}
          onClick={open}
        >
          {copy.open}
        </button>
        <input
          ref={input}
          type="file"
          accept="application/json,.json"
          aria-label={copy.open}
          hidden
          onChange={select}
        />
      </div>
      <div role="status" className={styles.status}>
        {pending && <p>{copy.replace}</p>}
        {status && <p>{status}</p>}
      </div>
      {pending && (
        <div className={styles.actions}>
          <button type="button" className={styles.action} onClick={apply}>
            {copy.apply}
          </button>
          <button type="button" className={styles.action} onClick={cancel}>
            {copy.cancel}
          </button>
        </div>
      )}
    </section>
  );
}
