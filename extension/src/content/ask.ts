import type { Reply, ToBackground } from "../shared/messages";
import { BrokerError } from "../shared/protocol";

/** Sends a request to the service worker; a failed reply becomes a BrokerError. */
export async function ask<T>(message: ToBackground): Promise<T> {
  const reply = (await chrome.runtime.sendMessage(message)) as Reply<T>;
  if (!reply.ok) throw new BrokerError(reply.code, reply.message);
  return reply.data;
}
