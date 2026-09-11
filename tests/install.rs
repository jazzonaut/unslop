//! The install button has to name a number before anything is fetched, so the
//! sizes are pinned in the source and formatted here.

use unslop::install::size;

#[test]
fn sizes_read_the_way_a_person_would_say_them() {
    // The three archives and the weights, as the button reports them.
    assert_eq!(size(31_666_541), "30MB");
    assert_eq!(size(254_078_211), "242MB");
    assert_eq!(size(254_078_211 + 391_443_627), "615MB");
    assert_eq!(size(3_525_956_768), "3.3GB");
    // Everything at once, which is what a genuinely first run offers.
    assert_eq!(size(3_525_956_768 + 254_078_211 + 391_443_627), "3.9GB");
}

#[test]
fn the_boundary_does_not_read_as_1024mb() {
    assert_eq!(size(1024 * 1024 * 1024 - 1), "1023MB");
    assert_eq!(size(1024 * 1024 * 1024), "1.0GB");
}
