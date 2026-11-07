
# This suffix is concatenated after the generated DSP, so `mydsp` is in scope.

def main() raises -> None:
    var dsp = unsafe_alloc[mydsp](1)
    dsp[] = mydsp()
    dsp[].init(SAMPLE_RATE)
    var ui = ImpulseGui()
    dsp[].build_user_interface(ui)
    run_impulse(dsp, ui)
    dsp.unsafe_free()
