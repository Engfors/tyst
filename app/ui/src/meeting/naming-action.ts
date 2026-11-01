// Decisions for the name prompt (SPEC 8.3). Escape asks before discarding. Enter and a second
// Escape stay on Keep, so a stray key never deletes the recording. An empty name saves as the
// default title. Closing the window and the countdown do not go through Escape.

export type NamingTarget = "input" | "keep" | "discard" | "other";

export type NamingDecision =
  | { type: "save"; title: string | null }
  | { type: "ask-discard" }
  | { type: "keep" }
  | { type: "discard" }
  | { type: "ignore" };

/** Empty and whitespace-only names save as the default title. */
export function titleToSave(title: string): string | null {
  const trimmed = title.trim();
  return trimmed ? trimmed : null;
}

/**
 * Blur and Save. A discard confirmation that is open does not save underneath itself;
 * cancelling it returns to the name that was typed. The countdown is separate: ignoring the
 * prompt still saves, including while the confirmation is open or the window is closed.
 */
export function implicitSave(
  confirming: boolean,
  title: string,
): { type: "save"; title: string | null } | { type: "ignore" } {
  if (confirming) return { type: "ignore" };
  return { type: "save", title: titleToSave(title) };
}

/**
 * Keys on the name prompt and on the discard confirmation.
 * While confirming, Enter discards only when Discard itself is focused. Every other Enter, and
 * Escape, keeps the recording and returns to naming.
 */
export function namingDecision(opts: {
  confirming: boolean;
  key: string;
  target: NamingTarget;
  title: string;
}): NamingDecision {
  if (opts.confirming) {
    if (opts.key === "Enter" && opts.target === "discard") return { type: "discard" };
    if (opts.key === "Enter" || opts.key === "Escape") return { type: "keep" };
    return { type: "ignore" };
  }
  if (opts.key === "Escape") return { type: "ask-discard" };
  if (opts.key === "Enter" && opts.target === "input") {
    return { type: "save", title: titleToSave(opts.title) };
  }
  return { type: "ignore" };
}
