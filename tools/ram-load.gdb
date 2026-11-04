# Run from the repository root, with the ELF supplied on the GDB command line.
# Requires a verified stock flash backup and a cold boot into ROM ISP.
# Stops at __bl706_main, before HAL clock or peripheral initialization.
set pagination off
set confirm off
set remotetimeout 10
target extended-remote localhost:3333
monitor halt
if $pc < 0x21000000 || $pc >= 0x21020000
    echo Refusing RAM load: the core is not executing in the BL706 ROM. Cold boot with minus held first.\n
    detach
    quit 1
end
load
compare-sections
set $pc = _start
thbreak __bl706_main
hbreak bl706_fatal_trap
continue
info registers pc sp gp mtvec mstatus
echo RAM startup stopped before HAL initialization. Inspect state before continuing.\n
