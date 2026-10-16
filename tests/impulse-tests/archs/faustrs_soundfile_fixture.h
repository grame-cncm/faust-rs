// Per-channel soundfile fixture for the C and C++ impulse lanes.
//
// Included right after the C++ harness's controlTools.h by faust-rs's copies
// of impulsearch.cpp and impulsearch2.cpp. The faust-rs generated code reads
// soundfile channel `chan % fChannels` (DIFF-BEH-016), and the `sound`
// reference gives each channel its own phase, sin(part + chan + 2*pi*i/L),
// as the TestMemoryReader of the C++ rework-soundfile change (5c1b9b257,
// master-dev) does. The pinned C++ branch (8eebea429) still gives every
// channel the same phase. This header makes the lanes independent of which
// one CPP_TESTS points at: it replaces the harness's reader and SoundUI,
// before main runs, with a reader that always writes the per-channel phase.
// The other lanes ship the same fixture in their own harnesses.

#ifndef FAUSTRS_SOUNDFILE_FIXTURE_H
#define FAUSTRS_SOUNDFILE_FIXTURE_H

struct FaustRsPerChannelMemoryReader : public TestMemoryReader {

    virtual void readFile(Soundfile* soundfile, const std::string& path_name, int part, int& offset, int max_chan)
    {
        soundfile->fLength[part] = SOUND_LENGTH;
        soundfile->fSR[part] = SOUND_SR;
        soundfile->fOffset[part] = offset;

        // Each channel has its own phase.
        if (soundfile->fIsDouble) {
            for (int sample = 0; sample < SOUND_LENGTH; sample++) {
                for (int chan = 0; chan < SOUND_CHAN; chan++) {
                    static_cast<double**>(soundfile->fBuffers)[chan][offset + sample] = std::sin(part + chan + (2 * M_PI * double(sample)/SOUND_LENGTH));
                }
            }
        } else {
            for (int sample = 0; sample < SOUND_LENGTH; sample++) {
                for (int chan = 0; chan < SOUND_CHAN; chan++) {
                    static_cast<float**>(soundfile->fBuffers)[chan][offset + sample] = std::sin(part + chan + (2 * M_PI * float(sample)/SOUND_LENGTH));
                }
            }
        }

        offset += SOUND_LENGTH;
    }
};

// controlTools.h builds `memory_reader` and `sound_ui` during static
// initialization; this definition follows theirs in the same translation unit,
// so it runs after them and both are replaced before any soundfile is read.
static bool faustrs_per_channel_soundfiles = []() {
    memory_reader = new FaustRsPerChannelMemoryReader();
    sound_ui = new SoundUI("", -1, memory_reader, (sizeof(FAUSTFLOAT) == sizeof(double)));
    return true;
}();

#endif
