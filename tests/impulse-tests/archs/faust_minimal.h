/* Minimal self-contained Faust architecture surface for the impulse drivers.
 * =========================================================================
 *
 * This is a faust-rs-owned reimplementation of just enough of the Faust
 * architecture contract for the generated code to compile and run: the `dsp`
 * base class, the abstract `UI` and `Meta` interfaces, the `Soundfile` layout
 * and the suite's synthetic soundfile fixture. It is deliberately NOT a copy of
 * the C++ Faust headers — it is
 * the subset the generated scalar code actually references, written here so the
 * `-ec` / `-os` targets need no C++ Faust checkout at all.
 *
 * Scope and non-goals: no polyphony, no MIDI, no real audio-file reading, no
 * OSC, no `libfaust`. Those are what make the reference 4-pass architecture
 * unvendorable (see `README.md`), and the scalar impulse pass needs none of
 * them. Anything a DSP could reference but this file does not implement is
 * either unused by the scalar pass or fails loudly at compile time rather than
 * silently changing results.
 *
 * The method set must stay a superset of what the C/C++ backends emit. If a
 * backend gains a new `ui_interface->` call or a new `dsp` virtual, the build
 * breaks here — which is the intended signal, not a maintenance burden to
 * avoid.
 */

#ifndef FAUST_RS_MINIMAL_ARCH_H
#define FAUST_RS_MINIMAL_ARCH_H

#include <cmath>
#include <string>
#include <vector>

#ifndef FAUSTFLOAT
#define FAUSTFLOAT float
#endif

/* ── Soundfile ──────────────────────────────────────────────────────────── */

/* Layout the generated code indexes directly:
 *   sample = ((FAUSTFLOAT**)fBuffers)[channel][fOffset[part] + i]
 * so the field order and types are a contract, not an implementation choice.
 * `fBuffers` is `void*` because the real Faust runtime picks `float**` or
 * `double**` at load time; here it is always `FAUSTFLOAT**`.
 */
struct Soundfile {
    void* fBuffers;  // FAUSTFLOAT** — MAX_CHAN non-interleaved channel arrays
    int* fLength;    // frames per part
    int* fSR;        // sample rate per part
    int* fOffset;    // offset of each part inside the global buffer
};

/* ── Metadata sink ──────────────────────────────────────────────────────── */

struct Meta {
    virtual ~Meta() {}
    virtual void declare(const char* key, const char* value) = 0;
};

/* ── User-interface sink ────────────────────────────────────────────────── */

struct UI {
    virtual ~UI() {}

    // -- layout
    virtual void openTabBox(const char* label) = 0;
    virtual void openHorizontalBox(const char* label) = 0;
    virtual void openVerticalBox(const char* label) = 0;
    virtual void closeBox() = 0;

    // -- active widgets
    virtual void addButton(const char* label, FAUSTFLOAT* zone) = 0;
    virtual void addCheckButton(const char* label, FAUSTFLOAT* zone) = 0;
    virtual void addVerticalSlider(const char* label, FAUSTFLOAT* zone, FAUSTFLOAT init,
                                   FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step) = 0;
    virtual void addHorizontalSlider(const char* label, FAUSTFLOAT* zone, FAUSTFLOAT init,
                                     FAUSTFLOAT min, FAUSTFLOAT max, FAUSTFLOAT step) = 0;
    virtual void addNumEntry(const char* label, FAUSTFLOAT* zone, FAUSTFLOAT init, FAUSTFLOAT min,
                             FAUSTFLOAT max, FAUSTFLOAT step) = 0;

    // -- passive widgets
    virtual void addHorizontalBargraph(const char* label, FAUSTFLOAT* zone, FAUSTFLOAT min,
                                       FAUSTFLOAT max) = 0;
    virtual void addVerticalBargraph(const char* label, FAUSTFLOAT* zone, FAUSTFLOAT min,
                                     FAUSTFLOAT max) = 0;

    // -- soundfiles
    virtual void addSoundfile(const char* label, const char* url, Soundfile** sf_zone) = 0;

    // -- metadata attached to a widget zone
    virtual void declare(FAUSTFLOAT* zone, const char* key, const char* value) = 0;
};

/* ── DSP base class ─────────────────────────────────────────────────────── */

class dsp {
   public:
    dsp() {}
    virtual ~dsp() {}

    virtual int getNumInputs() = 0;
    virtual int getNumOutputs() = 0;
    virtual void buildUserInterface(UI* ui_interface) = 0;
    virtual int getSampleRate() = 0;
    virtual void init(int sample_rate) = 0;
    virtual void instanceInit(int sample_rate) = 0;
    virtual void instanceConstants(int sample_rate) = 0;
    virtual void instanceResetUserInterface() = 0;
    virtual void instanceClear() = 0;
    virtual dsp* clone() = 0;
    virtual void metadata(Meta* m) = 0;
    virtual void compute(int count, FAUSTFLOAT** inputs, FAUSTFLOAT** outputs) = 0;
};


/* ── Shared soundfile fixture ───────────────────────────────────────────────
 *
 * The impulse suite does not read real audio files: every soundfile test is fed
 * one synthetic fixture, so results are reproducible without assets. This
 * mirrors `archs/impulserust.rs` and `tools/impulsewasm.js` exactly — a
 * divergence here would look like a codegen bug in `sound.dsp`:
 *
 *   - `SOUND_CHAN` real channels, both carrying the same signal;
 *   - `real_parts` parts of `SOUND_LENGTH` frames of sin(part + 2*pi*i/LENGTH);
 *   - the remaining parts up to `MAX_SOUNDFILE_PARTS` are empty and
 *     `SOUND_BUFFER_SIZE` long;
 *   - the `MAX_CHAN` channel-pointer table aliases the real channels modulo
 *     `SOUND_CHAN`, which is how a 4-output `soundfile` reads channels 2 and 3.
 */

#define MAX_CHAN 64
#define MAX_SOUNDFILE_PARTS 256

namespace faust_rs_soundfile {

const int kChannels = 2;
const int kLength = 4096;
const int kBufferSize = 1024;
const int kSampleRate = 44100;

/// Counts parts in a Faust URL literal (`{'a.wav';'b.wav'}`), like
/// `soundfilePartCount` in `tools/impulsewasm.js`.
inline int partCount(const char* url)
{
    if (url == nullptr) return 1;
    std::string text(url);
    std::string::size_type open = text.find('{');
    if (open == std::string::npos) return 1;
    std::string::size_type close = text.find('}', open + 1);
    if (close == std::string::npos) return 1;
    std::string body = text.substr(open + 1, close - open - 1);
    int count = 0;
    std::string::size_type start = 0;
    while (start <= body.size()) {
        std::string::size_type sep = body.find(';', start);
        std::string part = body.substr(start, (sep == std::string::npos) ? std::string::npos
                                                                         : sep - start);
        std::string::size_type first = part.find_first_not_of(" \t'");
        if (first != std::string::npos) count++;
        if (sep == std::string::npos) break;
        start = sep + 1;
    }
    return (count > 0) ? count : 1;
}

/// Owns one fixture's storage for the lifetime of the process.
struct Fixture {
    std::vector<std::vector<FAUSTFLOAT>> channels;
    std::vector<FAUSTFLOAT*> table;
    std::vector<int> lengths;
    std::vector<int> rates;
    std::vector<int> offsets;
    Soundfile soundfile;
};

inline Soundfile* make(int realParts)
{
    if (realParts > MAX_SOUNDFILE_PARTS) realParts = MAX_SOUNDFILE_PARTS;
    Fixture* fixture = new Fixture();

    int totalFrames = 0;
    for (int part = 0; part < realParts; part++) {
        fixture->offsets.push_back(totalFrames);
        fixture->lengths.push_back(kLength);
        totalFrames += kLength;
    }
    for (int part = realParts; part < MAX_SOUNDFILE_PARTS; part++) {
        fixture->offsets.push_back(totalFrames);
        fixture->lengths.push_back(kBufferSize);
        totalFrames += kBufferSize;
    }
    fixture->rates.assign(MAX_SOUNDFILE_PARTS, kSampleRate);

    fixture->channels.assign(kChannels, std::vector<FAUSTFLOAT>(totalFrames, FAUSTFLOAT(0)));
    for (int part = 0; part < realParts; part++) {
        const int offset = part * kLength;
        for (int sample = 0; sample < kLength; sample++) {
            const double value =
                std::sin(double(part) + (2.0 * 3.141592653589793 * double(sample)) / double(kLength));
            for (int channel = 0; channel < kChannels; channel++) {
                fixture->channels[channel][offset + sample] = FAUSTFLOAT(value);
            }
        }
    }

    fixture->table.resize(MAX_CHAN);
    for (int channel = 0; channel < MAX_CHAN; channel++) {
        fixture->table[channel] = fixture->channels[channel % kChannels].data();
    }

    fixture->soundfile.fBuffers = fixture->table.data();
    fixture->soundfile.fLength = fixture->lengths.data();
    fixture->soundfile.fSR = fixture->rates.data();
    fixture->soundfile.fOffset = fixture->offsets.data();
    return &fixture->soundfile;
}

}  // namespace faust_rs_soundfile

#endif  // FAUST_RS_MINIMAL_ARCH_H
