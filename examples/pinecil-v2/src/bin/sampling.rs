//! Dry run of timer-owned ADC/PWM scheduling. PWM1 is NOT routed to GPIO21.
#![no_std]
#![no_main]
use bl706_hal::{
    adc::{Adc, AnalogPin, Channel, Error, GainCalibration, Sample},
    gpio::Output,
    pwm::{Pwm, Unrouted},
    timer::{self, Compare, Events, Timer},
};
use core::fmt::Write;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use pinecil_v2_bringup::Logger;
use pinecil_v2_bringup::banner;
use static_cell::StaticCell;
type Inputs = (AnalogPin<18>, AnalogPin<19>, AnalogPin<20>);
const CHANNELS: [Channel; 8] = [
    Channel::Input10,
    Channel::Input9,
    Channel::Input8,
    Channel::Input9,
    Channel::Input10,
    Channel::Input9,
    Channel::Input8,
    Channel::Input9,
];
#[derive(Clone, Copy, Debug)]
struct Frame {
    sequence: u32,
    samples: [Sample; 8],
}
static RESULT: Signal<CriticalSectionRawMutex, Result<Frame, Error>> = Signal::new();
static DRIVER: StaticCell<timer::InterruptDriver<0, Sampler>> = StaticCell::new();
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
struct Sampler {
    adc: Adc<Inputs>,
    pwm: Pwm<1, Unrouted>,
    phase: u8,
    sequence: u32,
}
impl Sampler {
    fn step(&mut self, timer: &mut Timer<0>, events: Events) -> Result<(), Error> {
        // Coalesced/out-of-order events indicate lost timing. Stop instead of guessing order.
        let expected = [Compare::C0, Compare::C1, Compare::C2][self.phase as usize];
        if events != Events::compare(expected) {
            return Err(Error::Timeout);
        }
        let count = timer.counter();
        match self.phase {
            0 => {
                self.pwm.disable();
                if count >= 51_000 {
                    return Err(Error::Timeout);
                }
                self.phase = 1;
            }
            1 => {
                if count >= 60_000 {
                    return Err(Error::Timeout);
                }
                self.adc.start_prepared()?;
                self.phase = 2;
            }
            _ => {
                self.adc.stop();
                if count >= 50_000 {
                    return Err(Error::Timeout);
                }
                let mut samples = [Sample::default(); 8];
                for sample in &mut samples {
                    *sample = self.adc.try_read()?.ok_or(Error::ShortScan)?;
                }
                if self.adc.try_read()?.is_some() {
                    return Err(Error::ExtraData);
                }
                self.adc.prepare_scan(&CHANNELS)?;
                self.sequence = self.sequence.wrapping_add(1);
                RESULT.signal(Ok(Frame {
                    sequence: self.sequence,
                    samples,
                }));
                self.pwm.enable();
                self.phase = 0;
            }
        }
        Ok(())
    }
}
impl timer::Handler<0> for Sampler {
    fn on_interrupt(&mut self, timer: &mut Timer<0>, events: Events) {
        if let Err(error) = self.step(timer, events) {
            self.pwm.disable();
            self.adc.abort();
            timer.stop();
            timer.listen(Events::NONE);
            RESULT.signal(Err(error));
        }
    }
}
#[embassy_executor::task]
async fn report(mut log: Logger, _heater: Output<21>) {
    banner(
        &mut log,
        "TIMER0 sampling dry run: PWM1 unrouted, heater GPIO-low",
    );
    loop {
        match RESULT.wait().await {
            Ok(frame) => {
                let _ = writeln!(
                    log,
                    "sequence={} samples={:?}\r",
                    frame.sequence, frame.samples
                );
            }
            Err(error) => {
                let _ = writeln!(log, "sampling stopped: {error:?}\r");
            }
        }
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let mut log = Logger::init();
    let p = bl706_hal::init();
    let heater = p.pins.p21.into_output(false);
    let trim = GainCalibration::from_efuse_shadow();
    let _ = writeln!(log, "gain_trim={trim:?}\r");
    let mut adc = Adc::new(
        p.adc,
        (
            p.pins.p18.into_analog(),
            p.pins.p19.into_analog(),
            p.pins.p20.into_analog(),
        ),
        p.clocks,
        bl706_hal::adc::Config {
            calibration: trim.ok(),
            ..Default::default()
        },
    );
    adc.prepare_scan(&CHANNELS).unwrap();
    let mut pwm = Pwm::new(
        p.pwm1,
        Unrouted,
        p.clocks,
        bl706_hal::pwm::Config {
            divider: 60,
            period: 100,
        },
    )
    .unwrap();
    pwm.set_duty_cycle(50).unwrap();
    let timer = Timer::new(
        p.timer0,
        p.clocks,
        timer::Config {
            clock: timer::ClockSource::System,
            divider: 32,
            mode: timer::Mode::Preload {
                value: 0,
                trigger: Compare::C2,
            },
            compare: [50_000, 51_000, 60_000],
        },
    )
    .unwrap();
    DRIVER
        .init(timer::InterruptDriver::new(
            timer,
            Sampler {
                adc,
                pwm,
                phase: 0,
                sequence: 0,
            },
        ))
        .register(Events::ALL);
    bl706_hal::enable_interrupts();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(report(log, heater).unwrap());
    })
}
