import { useEffect, useState } from "react";
import { Lock } from "lucide-react";
import { api } from "../lib/api";
import { Button, ErrorText, Modal, PasswordInput } from "./ui";

/**
 * A browser asks "is it you?" (Linux has no Windows Hello): the master password answers it.
 * The password goes straight to Rust; five wrong tries or Cancel refuse the request.
 */
export function VerifyDialog({ request, onClose }: { request: { requestId: number; reason: string } | null; onClose: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = request?.requestId;

  // A new request starts with an empty field (the dialog is not remounted).
  useEffect(() => {
    setPassword("");
    setError(null);
    setBusy(false);
  }, [requestId]);

  const cancel = () => {
    if (request) api.brokerRespond(request.requestId, false).catch(() => {});
    onClose();
  };

  const submit = async () => {
    if (!request || !password || busy) return;
    setBusy(true);
    setError(null);
    try {
      const outcome = await api.brokerVerify(request.requestId, password);
      if (outcome === "wrong") {
        setError("That is not the master password.");
        setPassword("");
      } else {
        // Accepted: the browser goes on. Refused: too many tries, the browser was told no.
        onClose();
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={!!request}
      onClose={cancel}
      title="Confirm it is you"
      footer={
        <>
          <Button variant="ghost" onClick={cancel}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="verify-form" disabled={!password} busy={busy}>
            Confirm
          </Button>
        </>
      }
    >
      {request && (
        <form
          id="verify-form"
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <Lock className="mx-auto size-10 text-indigo-600" />
          <p className="text-center">
            Your browser asks for your vault: <b>{request.reason}</b>
          </p>
          <PasswordInput placeholder="Master password" value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
          {error && <ErrorText>{error}</ErrorText>}
          <p className="text-center text-sm text-slate-500">Confirm only if you just did this yourself.</p>
        </form>
      )}
    </Modal>
  );
}
