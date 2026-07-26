# Native lifecycle parity oracle

The `v1` and `v2` projects export the same lifecycle-bearing package as RPM,
Debian, and Arch artifacts. Their lifecycle programs record only source-ABI
observables: ordered event identity, script arguments, the package-script
stdin contract, and payload visibility at the event boundary.

Each focused CI target runs the fixture through its authoritative native
package manager:

- Fedora verifies the RPM traces.
- Ubuntu verifies the dpkg traces.
- Arch verifies the libalpm/pacman traces.

Those native runs must byte-match the files under `expected/`. Every target
then installs every source format through Conary and byte-matches the same
native-verified traces. The expected files are therefore reviewable contract
fixtures, not an independently invented Conary expectation.

Rollback is snapshot restoration, not a source-manager downgrade operation.
After rolling the v2 changeset back, the gate requires the exact v1 native
install trace and v1 payload again; removal must then extend that restored
trace with the native v1 removal event.
