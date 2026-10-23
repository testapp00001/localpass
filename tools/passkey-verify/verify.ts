// bun run verify.ts <fixture.json>
// Verifies a registration and a sign-in produced by `cargo run -p lp-engine --example passkey_fixture`.
import { readFileSync } from "node:fs";
import { verifyAuthenticationResponse, verifyRegistrationResponse } from "@simplewebauthn/server";

const fixture = JSON.parse(readFileSync(process.argv[2] ?? "fixture.json", "utf8"));
const { origin, rpId } = fixture;
let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};

const registration = await verifyRegistrationResponse({
  response: fixture.registration.response,
  expectedChallenge: fixture.registration.challenge,
  expectedOrigin: origin,
  expectedRPID: rpId,
  requireUserVerification: true,
});
check("registration verifies", registration.verified);
const info = registration.registrationInfo!;
check("credential id matches", info.credential.id === fixture.registration.response.id);
check("synced passkey flags", info.credentialDeviceType === "multiDevice" && info.credentialBackedUp);
check("attestation is none", info.fmt === "none");
check("counter starts at 0", info.credential.counter === 0);

const authentication = await verifyAuthenticationResponse({
  response: fixture.authentication.response,
  expectedChallenge: fixture.authentication.challenge,
  expectedOrigin: origin,
  expectedRPID: rpId,
  credential: info.credential,
  requireUserVerification: true,
});
check("sign-in verifies", authentication.verified);
check("counter stays 0", authentication.authenticationInfo.newCounter === 0);

// Negative checks: the same data must fail when anything is wrong.
const expectFailure = async (name: string, run: () => Promise<unknown>) => {
  // A bad signature comes back as verified: false; other problems throw.
  try {
    const result = (await run()) as { verified?: boolean };
    check(name, result?.verified === false, result?.verified ? "was accepted" : "");
  } catch {
    check(name, true);
  }
};
const flip = (b64: string) => {
  const bytes = Buffer.from(b64, "base64url");
  bytes[bytes.length - 1] ^= 1;
  return bytes.toString("base64url");
};
const auth = fixture.authentication.response;
await expectFailure("tampered signature is rejected", () =>
  verifyAuthenticationResponse({
    response: { ...auth, response: { ...auth.response, signature: flip(auth.response.signature) } },
    expectedChallenge: fixture.authentication.challenge,
    expectedOrigin: origin,
    expectedRPID: rpId,
    credential: info.credential,
  }),
);
await expectFailure("wrong challenge is rejected", () =>
  verifyAuthenticationResponse({
    response: auth,
    expectedChallenge: fixture.registration.challenge,
    expectedOrigin: origin,
    expectedRPID: rpId,
    credential: info.credential,
  }),
);
await expectFailure("wrong origin is rejected", () =>
  verifyAuthenticationResponse({
    response: auth,
    expectedChallenge: fixture.authentication.challenge,
    expectedOrigin: "https://evil.example.net",
    expectedRPID: rpId,
    credential: info.credential,
  }),
);
await expectFailure("wrong relying party is rejected", () =>
  verifyAuthenticationResponse({
    response: auth,
    expectedChallenge: fixture.authentication.challenge,
    expectedOrigin: origin,
    expectedRPID: "example.org",
    credential: info.credential,
  }),
);

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("all checks passed");
