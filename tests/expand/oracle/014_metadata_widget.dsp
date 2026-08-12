declare version "<version>";
declare compile_options "<options>";
declare filename "014_metadata_widget.dsp";
declare name "014_metadata_widget";
ID_0 = hslider("[unit:dB]gain", 0.0f, -7e+01f, 0.0f, 0.1f);
ID_1 = _, ID_0;
ID_2 = ID_1 : *;
process = ID_2;
