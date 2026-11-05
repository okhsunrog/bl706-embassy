# Vendor controller and RF pair

Original archives from Bouffalo Lab's
[`bl_iot_sdk-components`](https://github.com/bouffalolab/bl_iot_sdk-components/tree/a18c0f9215a9490ca2ce4811084b96ee60e4f205):

- `network/ble/blecontroller_bl702_m0s1/lib/libblecontroller_bl702_m0s1.a`
- `platform/soc/bl702/bl702_rf/lib/libbl702_rf.a`

The build script verifies their SHA256 values and removes `arch_main.o` only
from the build copy, replacing the FreeRTOS wrapper with the Rust port. The
original files here are unchanged. The SDK license is included as LICENSE-APACHE;
the source, hashes and analysis are recorded in `docs/ble-controller.md`.
