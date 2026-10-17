This is a versioned replacement input bundle for the stopped-recovery audit case.
The historical `Cases` assertions are unchanged; fixture setup is self-contained
and supplies a usable acceptance policy. The old frozen baseline and historical
runtime records are retained. The checker no longer imports a helper generated
only during host verification.

For a future authorized goal, the owner copies and pins this entire directory
through the existing read-only `data` mount, outside the candidate's writable
source. From the assigned repository root, the worker can run `data/check.py`
with Python. Independent verification runs the trusted bundle's `verify.py`
directly with Python and the existing exact `FACTORY_VERIFICATION_COMMIT` binding.
No shell pipeline or command override is accepted. Full child diagnostics are
retained, nonzero status propagates, and a zero exit needs the expected positive
unittest result before structured evidence is emitted.

`manifest.json` binds the checker bytes. It is not a substitute for the owner's
existing trusted-file pins on the verifier and inputs. Do not verify a candidate
using its writable copy of this bundle. These files do not activate or migrate
the paused goal; adoption requires a separately authorized exact configuration.

The local qualification stages a separate input directory with a worker `data`
symlink and real repository scripts. It exercises the historical bad
audit branch, missing imports, passing checks, integrity failures and misleading
trailing output. Native effects remain injected; it invokes no model or runtime.
