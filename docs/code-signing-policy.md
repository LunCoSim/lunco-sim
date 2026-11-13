# LunCoSim code-signing policy

This policy covers Windows x86_64 LunCoSim releases built by the manual
[`Dev Builds` GitHub Actions workflow](../.github/workflows/dev.yml).

## Production releases

- The workflow must run from the `main` branch and build the release from the
  checked-out LunCoSim source.
- One SignPath request signs the LunCoSim executable, the executable embedded
  in the MSI, and the MSI. Velopack packages are then built from that same
  signed executable so the Windows updater receives the signed binary.
- Each production signing request requires manual approval in SignPath by a
  LunCoSim maintainer assigned the project approver role. The approver checks
  the source commit, workflow run, and artifact before approving it.
- Changes to build scripts, the signing configuration, and release workflow
  receive the same source review as application code.
- The production signing policy accepts builds from the configured GitHub
  trusted build system and the `main` branch. Locally built or manually
  uploaded artifacts are not production-signed.

## Test builds

Test signing uses a separate test policy and certificate. Test-signed files are
published only as temporary GitHub Actions artifacts and never as GitHub
releases or updater packages. A self-signed test certificate is not trusted by
Windows by default; only test it on an isolated Windows machine after
installing that certificate in the appropriate trust stores.

## Signature coverage

The production SignPath artifact configuration checks the executable's product
name, product version, file version, company name, and original filename before
signing. Windows release verification checks the Authenticode signatures on
the standalone executable, MSI, and executable inside the full Velopack update
package.

The macOS and Linux installers are not covered by this Windows Authenticode
policy.

## Verify a Windows release

In PowerShell, inspect the executable or MSI downloaded from the linked GitHub
release:

```powershell
Get-AuthenticodeSignature .\luncosim.exe | Format-List Status,SignerCertificate
```

The `Status` should be `Valid`. Confirm the certificate subject against the
publisher shown for that release; the certificate identity is the authoritative
publisher name.
