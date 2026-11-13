# Testing OpenDiag on a real vehicle

OpenDiag has **never been run against a real car**. Everything so far was tested against simulators and a few
published captures, so the most useful thing anyone with a vehicle can do is a careful, **read-only** test and a
report. Thank you. This page is the safe way to do it.

## Ground rules

- **Read-only only.** Use `scan`, `dtc read`, `log`, `report`, `probe` and `capture`. **Do not** run `dtc clear`,
  `adaptation set` or `backup restore` until reads on your module have worked reliably several times, and only
  if you understand what they change. Nothing here flashes or reprograms anything.
- **Ignition on, engine off** (or a bench module on a 12 V supply). Keep the battery healthy; a long session on a
  weak battery can upset modules.
- **Always add `--trace <file>`.** A failure with a trace is debuggable; without one it mostly isn't.
- You use this at your own risk; see the licence. Older modules are tolerant, but we cannot promise anything yet.

## What you need

- A supported connection: a K-line cable (FTDI-style KKL) for older cars, SocketCAN on Linux for TP2.0, or an
  ELM327-style adapter for UDS over CAN (see [elm327.md](elm327.md); degraded). Check the vehicle really speaks
  what you expect before blaming the software.
- The release archive, unpacked ([releases](https://github.com/JoeeDeveloper/OpenDiag/releases)). Run it from
  inside the folder. In PowerShell use `.\opendiag.exe`.

## Steps

1. `opendiag doctor --out doctor.txt` shows your version, OS and serial ports and checks the built-in virtual
   modules. It contacts nothing and contains no VIN. Attach it to your report.
2. `opendiag ports`: does your cable appear? Note the port name (for example `COM3` or `/dev/ttyUSB0`).
3. `opendiag probe --port <PORT> --address 0x01 --slow-init --trace probe.trace`. Older modules often need a few
   tries; try `--baud 9600`, then `--no-echo`.
4. If that connects: `opendiag scan --port <PORT> --slow-init --trace scan.trace`, then
   `opendiag dtc read --port <PORT> --slow-init --trace dtc.trace`.
5. Optional: `opendiag log --port <PORT> --slow-init --groups 1 --samples 5 --trace log.trace`.
6. Newer K-line modules use `--fast-init` instead of `--slow-init`. CAN: see [status.md](status.md).

If a step fails, stop there and report what happened. Do not keep retrying against a car for long.

## What to send

Open a **Hardware test report** issue and include:

- the exact commands and the console output (copy it as it is, including errors);
- `doctor.txt`;
- the `.trace` files. **Read them first**: they can contain your VIN and other identification
  ([capture.md](capture.md)). Remove or redact anything you do not want public;
- the car (make, model, year, engine), the module address, the cable or adapter model and where you plugged in.

Successes are just as valuable as failures: "it worked on a 2003 Golf 4 engine ECU with cable X" tells us what is
verified. If you are willing, a trace of a successful read can become a regression test for the project.

## Passive captures

If you already own a tool that talks to your car, `opendiag capture` can record traffic without sending
anything (read [capture.md](capture.md) first). Such traces are how the protocol code gets checked against
reality. Only share captures you made yourself from your own vehicle.
