import type { DecisionRequest, Reply, ToBackground } from "../shared/messages";
import { BrokerError } from "../shared/protocol";

/** Sends a request to the service worker; a failed reply becomes a BrokerError. */
export async function ask<T>(message: ToBackground): Promise<T> {
  const reply = (await chrome.runtime.sendMessage(message)) as Reply<T>;
  if (!reply.ok) throw new BrokerError(reply.code, reply.message);
  return reply.data;
}

/** How often an open `decide` call says it is still there (tests shorten it). */
export const decideTiming = { pingMs: 10_000 };

/**
 * Asks the background for a decision in LocalPass UI and waits for its outcome. While the call is open
 * no LocalPass panel may be showing (a save or passkey request waits with a badge), and Firefox may
 * suspend an idle event page and forget the request: a small message every `pingMs` keeps it awake.
 */
export async function askDecision<T>(ref: string, request: DecisionRequest): Promise<T> {
  const timer = setInterval(() => void ask({ kind: "decidePing", ref }).catch(() => {}), decideTiming.pingMs);
  try {
    return await ask<T>({ kind: "decide", ref, request });
  } finally {
    clearInterval(timer);
  }
}
