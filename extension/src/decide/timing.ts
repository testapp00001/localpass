// Timing rules of the decision window; tests shorten them.
export const timing = {
  /**
   * How long after the window shows something (or regains focus) its action buttons stay disabled. A
   * page can choose the moment the window opens (a passkey call from script, or a replacement request),
   * so a user who is typing or pressing Space or Enter must not land on an action: the same reason Firefox
   * delays its own security prompts, and what `holdMs` does for the in-page controls in Chrome.
   */
  holdMs: 600,
  /** Firefox suspends the background after about 30 s without events; a message this often keeps it awake. */
  keepAliveMs: 10_000,
};
