//! Read-only analog and unrouted PWM/timer IRQ bring-up. Heater stays GPIO-low.
#![no_std]
#![no_main]
use bl706_hal::{
    adc::{Adc, AnalogPin, Channel, Config as AdcConfig, GainCalibration, Sample},
    gpio::Output,
    pwm::{Config as PwmConfig, Pwm, Unrouted},
    timer::{ClockSource, Compare, Config as TimerConfig, Mode, Timer as HardwareTimer},
    uart::Uart0,
};
use core::fmt::Write;
use embassy_time::{Duration, Timer, with_timeout};
use pinecil_v2_bringup::banner;
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
type Inputs = (AnalogPin<18>, AnalogPin<19>, AnalogPin<20>);

#[embassy_executor::task]
async fn check(
    mut uart: Uart0,
    _heater: Output<21>,
    mut adc: Adc<Inputs>,
    mut timer: HardwareTimer<1>,
    mut pwm: Pwm<0, Unrouted>,
) {
    banner(&mut uart, "ADC scan / TIMER1 / unrouted PWM0 IRQ");
    let _ = writeln!(uart, "calibration={:?}\r", adc.config().calibration);
    timer.start();
    pwm.enable();
    loop {
        let timer_result =
            with_timeout(Duration::from_secs(1), timer.wait_for_compare(Compare::C0)).await;
        let pwm_result = with_timeout(Duration::from_millis(100), pwm.wait_periods(10)).await;
        let mut samples = [Sample::default(); 8];
        let result = adc
            .read_scan(
                &[
                    Channel::Input10,
                    Channel::Input9,
                    Channel::Input8,
                    Channel::Input9,
                    Channel::Input10,
                    Channel::Input9,
                    Channel::Input8,
                    Channel::Input9,
                ],
                &mut samples,
                Duration::from_millis(100),
            )
            .await;
        let _ = writeln!(
            uart,
            "timer={timer_result:?} counter={} pwm={pwm_result:?} hz={} adc={result:?} samples={samples:?}\r",
            timer.counter(),
            pwm.frequency_hz()
        );
        Timer::after_millis(500).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let p = bl706_hal::init();
    let heater = p.pins.p21.into_output(false);
    let mut uart = Uart0::new(p.uart0, p.pins.p22, p.clocks, 115_200).unwrap();
    let trim = GainCalibration::from_efuse_shadow();
    let _ = writeln!(uart, "gain_trim={trim:?}\r");
    let calibration = trim.ok();
    let adc = Adc::new(
        p.adc,
        (
            p.pins.p18.into_analog(),
            p.pins.p19.into_analog(),
            p.pins.p20.into_analog(),
        ),
        p.clocks,
        AdcConfig {
            calibration,
            ..Default::default()
        },
    );
    let timer = HardwareTimer::new(
        p.timer1,
        p.clocks,
        TimerConfig {
            clock: ClockSource::System,
            divider: 32,
            mode: Mode::Preload {
                value: 0,
                trigger: Compare::C0,
            },
            compare: [200_000, u32::MAX, u32::MAX],
        },
    )
    .unwrap();
    let pwm = Pwm::new(
        p.pwm0,
        Unrouted,
        p.clocks,
        PwmConfig {
            divider: 32,
            period: 1000,
        },
    )
    .unwrap();
    bl706_hal::enable_interrupts();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(check(uart, heater, adc, timer, pwm).unwrap());
    })
}
