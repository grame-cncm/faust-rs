/* Shared body of the -ec / -os scalar impulse driver.
 * =================================================
 *
 * Included by `impulseexecopts.cpp` (C++ backend: the generated class is used
 * directly) and `impulseexecopts_c.cpp` (C backend: a thin wrapper class
 * forwards to the generated C functions). Everything that differs between the
 * two lives in those two files; the driver itself is here, once.
 *
 * The driver expects `FAUSTCLASS` to offer `init`, `getNumInputs`,
 * `getNumOutputs`, `buildUserInterface`, `compute`, and — depending on the
 * selected shape — `control` and `frame`.
 */

/* Must match the reference architecture's `kFrames` (controlTools.h): the block
 * size decides how long the buttons stay pressed — they are held for the first
 * block only. A larger block here holds them longer and `UITester.dsp` (two
 * `button` widgets) then differs from the reference from frame 64 onward. */
#define kFrames 64

/* Collects the button zones so the driver can press them on the first cycle,
 * exactly as the reference architecture does.
 *
 * Buttons only: the reference's `FUI::addButton` registers its zone for
 * `setButtons()` while `FUI::addCheckButton` does not, so a check button keeps
 * the value the generated `instanceResetUserInterface()` gave it. Pressing
 * check buttons here too makes `UITester.dsp` differ from the reference by a
 * constant 1 — which is how this was found. */
struct ButtonUI : public UI {
    std::vector<FAUSTFLOAT*> fButtons;

    void openTabBox(const char*) override {}
    void openHorizontalBox(const char*) override {}
    void openVerticalBox(const char*) override {}
    void closeBox() override {}

    void addButton(const char*, FAUSTFLOAT* zone) override { fButtons.push_back(zone); }
    void addCheckButton(const char*, FAUSTFLOAT*) override {}

    void addVerticalSlider(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT,
                           FAUSTFLOAT) override {}
    void addHorizontalSlider(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT,
                             FAUSTFLOAT) override {}
    void addNumEntry(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT, FAUSTFLOAT,
                     FAUSTFLOAT) override {}
    void addHorizontalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) override {}
    void addVerticalBargraph(const char*, FAUSTFLOAT*, FAUSTFLOAT, FAUSTFLOAT) override {}
    /* Installs the shared synthetic fixture, like the reference harness: the
     * suite reads no real audio assets. */
    void addSoundfile(const char*, const char* url, Soundfile** sf_zone) override
    {
        *sf_zone = faust_rs_soundfile::make(faust_rs_soundfile::partCount(url));
    }
    void declare(FAUSTFLOAT*, const char*, const char*) override {}

    void press(FAUSTFLOAT value)
    {
        for (FAUSTFLOAT* zone : fButtons) {
            *zone = value;
        }
    }
};

/* Mirrors the reference's -0.0 handling so the printed text can be compared
 * byte for byte rather than numerically. */
static FAUSTFLOAT normalize(FAUSTFLOAT value)
{
    if (std::isnan(value) || std::isinf(value)) {
        return value;
    }
    return (value == 0.0) ? FAUSTFLOAT(0.0) : value;
}

int main(int argc, char* argv[])
{
    int frames = 60000;
    for (int i = 1; i < argc; i++) {
        if (std::strcmp(argv[i], "-n") == 0 && i + 1 < argc) {
            frames = std::atoi(argv[++i]);
        }
    }

    // Heap-allocated like the reference architecture: DSPs with long delay
    // lines overflow a default stack frame.
    FAUSTCLASS* dsp = new FAUSTCLASS();
    dsp->init(44100);

    const int numInputs = dsp->getNumInputs();
    const int numOutputs = dsp->getNumOutputs();

    printf("number_of_inputs  : %3d\n", numInputs);
    printf("number_of_outputs : %3d\n", numOutputs);
    printf("number_of_frames  : %6d\n", frames);

    std::vector<std::vector<FAUSTFLOAT>> inputStore(numInputs,
                                                    std::vector<FAUSTFLOAT>(kFrames, 0.0));
    std::vector<std::vector<FAUSTFLOAT>> outputStore(numOutputs,
                                                     std::vector<FAUSTFLOAT>(kFrames, 0.0));
    std::vector<FAUSTFLOAT*> inputs(numInputs);
    std::vector<FAUSTFLOAT*> outputs(numOutputs);
    for (int c = 0; c < numInputs; c++) inputs[c] = inputStore[c].data();
    for (int c = 0; c < numOutputs; c++) outputs[c] = outputStore[c].data();

    ButtonUI buttons;
    dsp->buildUserInterface(&buttons);

#ifdef IMPULSE_OS
    // One-sample mode reads and writes flat per-frame arrays, not channel
    // pointers, so the frame slices are staged separately.
    std::vector<FAUSTFLOAT> frameIn(numInputs > 0 ? numInputs : 1, 0.0);
    std::vector<FAUSTFLOAT> frameOut(numOutputs > 0 ? numOutputs : 1, 0.0);
#endif

    int written = 0;
    int cycle = 0;
    while (written < frames) {
        const int count = (frames - written < kFrames) ? (frames - written) : kFrames;

        for (int c = 0; c < numInputs; c++) {
            std::fill(inputStore[c].begin(), inputStore[c].end(), FAUSTFLOAT(0.0));
        }
        for (int c = 0; c < numOutputs; c++) {
            std::fill(outputStore[c].begin(), outputStore[c].end(), FAUSTFLOAT(0.0));
        }
        if (written == 0) {
            for (int c = 0; c < numInputs; c++) inputStore[c][0] = FAUSTFLOAT(1.0);
        }
        buttons.press(cycle == 0 ? FAUSTFLOAT(1.0) : FAUSTFLOAT(0.0));

#ifdef IMPULSE_EC
        // Under -ec every block-rate value lives in `control()`: skipping it
        // is exactly the failure mode this target exists to catch.
        dsp->control();
#endif

#ifdef IMPULSE_OS
        for (int frame = 0; frame < count; frame++) {
            for (int c = 0; c < numInputs; c++) frameIn[c] = inputStore[c][frame];
            dsp->frame(frameIn.data(), frameOut.data());
            for (int c = 0; c < numOutputs; c++) outputStore[c][frame] = frameOut[c];
        }
#else
        dsp->compute(count, inputs.data(), outputs.data());
#endif

        for (int frame = 0; frame < count; frame++) {
            printf("%6d :", written);
            for (int c = 0; c < numOutputs; c++) {
                printf("  %.6f", normalize(outputStore[c][frame]));
            }
            printf("\n");
            written++;
        }
        cycle++;
    }

    delete dsp;
    return 0;
}
