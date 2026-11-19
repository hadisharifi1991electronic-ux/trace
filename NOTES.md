
3D FEM via model:

- Differential pairs: `trace via ... --pair P --check fem` solves the
  pair with an odd-mode wave port (Sdd11/Sdd21, Z0 differential), each
  via its own conductor; oval antipads (`--oval`) as one capsule hole.
- Thick copper: each trace is its real thickness of perfect conductor, so
  line impedances now match the stack-up (CutiePi 50 ohm line: 47.6-52.7
  ohm in FEM, was 55-58 as zero-thickness sheets).
- `--fem-dof N` caps the unknowns (memory: roughly 40 kB each).
- Port Z0 must settle within 0.5% between passes (8 mil stripline: 55.9
  against 56.5 ohm exact).
- Starting grid grades less (thin copper no longer fills the box with
  fine lines).

