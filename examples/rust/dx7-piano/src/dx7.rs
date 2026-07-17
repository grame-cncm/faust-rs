/* ------------------------------------------------------------
Code generated with faust-rs
Compilation options: -lang rust
------------------------------------------------------------ */

pub type FaustFloat = f32;

#[allow(dead_code)]
fn remainder_f32(x: f32, y: f32) -> f32 {
    x - y * (x / y).round_ties_even()
}

#[allow(dead_code)]
fn remainder_f64(x: f64, y: f64) -> f64 {
    x - y * (x / y).round_ties_even()
}

#[allow(non_upper_case_globals, dead_code)]
static iTbl59: [i32; 20] = [0i32, 5i32, 9i32, 13i32, 17i32, 20i32, 23i32, 25i32, 27i32, 29i32, 31i32, 33i32, 35i32, 37i32, 39i32, 41i32, 42i32, 43i32, 45i32, 46i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl242: [i32; 64] = [0i32, 70i32, 86i32, 97i32, 106i32, 114i32, 121i32, 126i32, 132i32, 138i32, 142i32, 148i32, 152i32, 156i32, 160i32, 163i32, 166i32, 170i32, 173i32, 174i32, 178i32, 181i32, 184i32, 186i32, 189i32, 190i32, 194i32, 196i32, 198i32, 200i32, 202i32, 205i32, 206i32, 209i32, 211i32, 214i32, 216i32, 218i32, 220i32, 222i32, 224i32, 225i32, 227i32, 229i32, 230i32, 232i32, 233i32, 235i32, 237i32, 238i32, 240i32, 241i32, 242i32, 243i32, 244i32, 246i32, 246i32, 248i32, 249i32, 250i32, 251i32, 252i32, 253i32, 254i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl129: [i32; 33] = [0i32, 1i32, 2i32, 3i32, 4i32, 5i32, 6i32, 7i32, 8i32, 9i32, 11i32, 14i32, 16i32, 19i32, 23i32, 27i32, 33i32, 39i32, 47i32, 56i32, 66i32, 80i32, 94i32, 110i32, 126i32, 142i32, 158i32, 174i32, 190i32, 206i32, 222i32, 238i32, 250i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl382: [i32; 77] = [1764000i32, 1764000i32, 1411200i32, 1411200i32, 1190700i32, 1014300i32, 992250i32, 882000i32, 705600i32, 705600i32, 584325i32, 507150i32, 502740i32, 441000i32, 418950i32, 352800i32, 308700i32, 286650i32, 253575i32, 220500i32, 220500i32, 176400i32, 145530i32, 145530i32, 125685i32, 110250i32, 110250i32, 88200i32, 88200i32, 74970i32, 61740i32, 61740i32, 55125i32, 48510i32, 44100i32, 37485i32, 31311i32, 30870i32, 27562i32, 27562i32, 22050i32, 18522i32, 17640i32, 15435i32, 14112i32, 13230i32, 11025i32, 9261i32, 9261i32, 7717i32, 6615i32, 6615i32, 5512i32, 5512i32, 4410i32, 3969i32, 3969i32, 3439i32, 2866i32, 2690i32, 2249i32, 1984i32, 1896i32, 1808i32, 1411i32, 1367i32, 1234i32, 1146i32, 926i32, 837i32, 837i32, 705i32, 573i32, 573i32, 529i32, 441i32, 441i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl734: [i32; 4] = [0i32, 4342338i32, 7171437i32, 16777216i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl938: [i32; 32] = [-16777216i32, 0i32, 16777216i32, 26591258i32, 33554432i32, 38955489i32, 43368474i32, 47099600i32, 50331648i32, 53182516i32, 55732705i32, 58039632i32, 60145690i32, 62083076i32, 63876816i32, 65546747i32, 67108864i32, 68576247i32, 69959732i32, 71268397i32, 72509921i32, 73690858i32, 74816848i32, 75892776i32, 76922906i32, 77910978i32, 78860292i32, 79773775i32, 80654032i32, 81503396i32, 82323963i32, 83117622i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl1047: [i32; 100] = [-128i32, -116i32, -104i32, -95i32, -85i32, -76i32, -68i32, -61i32, -56i32, -52i32, -49i32, -46i32, -43i32, -41i32, -39i32, -37i32, -35i32, -33i32, -32i32, -31i32, -30i32, -29i32, -28i32, -27i32, -26i32, -25i32, -24i32, -23i32, -22i32, -21i32, -20i32, -19i32, -18i32, -17i32, -16i32, -15i32, -14i32, -13i32, -12i32, -11i32, -10i32, -9i32, -8i32, -7i32, -6i32, -5i32, -4i32, -3i32, -2i32, -1i32, 0i32, 1i32, 2i32, 3i32, 4i32, 5i32, 6i32, 7i32, 8i32, 9i32, 10i32, 11i32, 12i32, 13i32, 14i32, 15i32, 16i32, 17i32, 18i32, 19i32, 20i32, 21i32, 22i32, 23i32, 24i32, 25i32, 26i32, 27i32, 28i32, 29i32, 30i32, 31i32, 32i32, 33i32, 34i32, 35i32, 38i32, 40i32, 43i32, 46i32, 49i32, 53i32, 58i32, 65i32, 73i32, 82i32, 92i32, 103i32, 115i32, 127i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl1102: [i32; 100] = [1i32, 2i32, 3i32, 3i32, 4i32, 4i32, 5i32, 5i32, 6i32, 6i32, 7i32, 7i32, 8i32, 8i32, 9i32, 9i32, 10i32, 10i32, 11i32, 11i32, 12i32, 12i32, 13i32, 13i32, 14i32, 14i32, 15i32, 16i32, 16i32, 17i32, 18i32, 18i32, 19i32, 20i32, 21i32, 22i32, 23i32, 24i32, 25i32, 26i32, 27i32, 28i32, 30i32, 31i32, 33i32, 34i32, 36i32, 37i32, 38i32, 39i32, 41i32, 42i32, 44i32, 46i32, 47i32, 49i32, 51i32, 53i32, 54i32, 56i32, 58i32, 60i32, 62i32, 64i32, 66i32, 68i32, 70i32, 72i32, 74i32, 76i32, 79i32, 82i32, 85i32, 88i32, 91i32, 94i32, 98i32, 102i32, 106i32, 110i32, 115i32, 120i32, 125i32, 130i32, 135i32, 141i32, 147i32, 153i32, 159i32, 165i32, 171i32, 178i32, 185i32, 193i32, 202i32, 211i32, 232i32, 243i32, 254i32, 255i32];
#[allow(non_upper_case_globals, dead_code)]
static iTbl1202: [i32; 8] = [0i32, 10i32, 20i32, 33i32, 55i32, 92i32, 153i32, 255i32];

#[allow(non_snake_case, non_camel_case_types, dead_code)]
pub struct Dx7Piano {
    fSampleRate: i32,
    fVec12: [f32; 2],
    iRec15971: [i32; 2],
    iRec15971_1: [i32; 2],
    iRec15971_2: [i32; 2],
    iRec15971_3: [i32; 2],
    iRec15971_4: [i32; 2],
    iRec15971_5: [i32; 2],
    iRec15971_6: [i32; 2],
    fButton19: FaustFloat,
    fHslider44: FaustFloat,
    fHslider51: FaustFloat,
    fHslider13: FaustFloat,
    fHslider45: FaustFloat,
    fHslider6: FaustFloat,
    fHslider5: FaustFloat,
    fHslider46: FaustFloat,
    fEntry49: FaustFloat,
    fHslider50: FaustFloat,
    fEntry47: FaustFloat,
    fHslider48: FaustFloat,
    fConst1: f32,
    fHslider55: FaustFloat,
    fHslider56: FaustFloat,
    fHslider41: FaustFloat,
    fHslider52: FaustFloat,
    fHslider43: FaustFloat,
    fHslider42: FaustFloat,
    fHslider54: FaustFloat,
    fHslider53: FaustFloat,
    fHslider57: FaustFloat,
    fHslider21: FaustFloat,
    fRec16024: [f32; 2],
    fRec16024_1: [f32; 2],
    fRec16024_2: [f32; 2],
    fRec16024_3: [f32; 2],
    iRec16024_4: [i32; 2],
    fRec16024_5: [f32; 2],
    fRec16024_6: [f32; 2],
    fConst2: f32,
    fHslider23: FaustFloat,
    fHslider22: FaustFloat,
    fHslider24: FaustFloat,
    fConst3: f32,
    fEntry25: FaustFloat,
    fRec17426: f32,
    fHslider26: FaustFloat,
    fCheckbox58: FaustFloat,
    fHslider59: FaustFloat,
    fHslider61: FaustFloat,
    fHslider60: FaustFloat,
    fRec16090: [f32; 2],
    iRec16090_1: [i32; 2],
    iRec16090_2: [i32; 2],
    iRec16090_3: [i32; 2],
    fRec16090_4: [f32; 2],
    iRec16090_5: [i32; 2],
    fHslider31: FaustFloat,
    fHslider34: FaustFloat,
    fConst4: f32,
    fHslider35: FaustFloat,
    fHslider38: FaustFloat,
    fHslider33: FaustFloat,
    fHslider32: FaustFloat,
    fHslider37: FaustFloat,
    fHslider36: FaustFloat,
    fHslider39: FaustFloat,
    fHslider40: FaustFloat,
    iRec16339: [i32; 2],
    iRec16339_1: [i32; 2],
    iRec16339_2: [i32; 2],
    iRec16339_3: [i32; 2],
    iRec16339_4: [i32; 2],
    iRec16339_5: [i32; 2],
    iRec16339_6: [i32; 2],
    fHslider3: FaustFloat,
    fHslider12: FaustFloat,
    fHslider4: FaustFloat,
    fHslider7: FaustFloat,
    fEntry10: FaustFloat,
    fHslider11: FaustFloat,
    fEntry8: FaustFloat,
    fHslider9: FaustFloat,
    fHslider17: FaustFloat,
    fHslider18: FaustFloat,
    fHslider0: FaustFloat,
    fHslider14: FaustFloat,
    fHslider2: FaustFloat,
    fHslider1: FaustFloat,
    fHslider16: FaustFloat,
    fHslider15: FaustFloat,
    fHslider20: FaustFloat,
    fRec17443: f32,
    fCheckbox27: FaustFloat,
    fHslider28: FaustFloat,
    fHslider30: FaustFloat,
    fHslider29: FaustFloat,
    iRec16599: [i32; 2],
    iRec16599_1: [i32; 2],
    iRec16599_2: [i32; 2],
    iRec16599_3: [i32; 2],
    iRec16599_4: [i32; 2],
    iRec16599_5: [i32; 2],
    iRec16599_6: [i32; 2],
    fHslider86: FaustFloat,
    fHslider93: FaustFloat,
    fHslider87: FaustFloat,
    fHslider88: FaustFloat,
    fEntry91: FaustFloat,
    fHslider92: FaustFloat,
    fEntry89: FaustFloat,
    fHslider90: FaustFloat,
    fHslider97: FaustFloat,
    fHslider98: FaustFloat,
    fHslider83: FaustFloat,
    fHslider94: FaustFloat,
    fHslider85: FaustFloat,
    fHslider84: FaustFloat,
    fHslider96: FaustFloat,
    fHslider95: FaustFloat,
    fHslider99: FaustFloat,
    fRec17468: f32,
    fCheckbox100: FaustFloat,
    fHslider101: FaustFloat,
    fHslider103: FaustFloat,
    fHslider102: FaustFloat,
    iRec16851: [i32; 2],
    iRec16851_1: [i32; 2],
    iRec16851_2: [i32; 2],
    iRec16851_3: [i32; 2],
    iRec16851_4: [i32; 2],
    iRec16851_5: [i32; 2],
    iRec16851_6: [i32; 2],
    fHslider65: FaustFloat,
    fHslider72: FaustFloat,
    fHslider66: FaustFloat,
    fHslider67: FaustFloat,
    fEntry70: FaustFloat,
    fHslider71: FaustFloat,
    fEntry68: FaustFloat,
    fHslider69: FaustFloat,
    fHslider76: FaustFloat,
    fHslider77: FaustFloat,
    fHslider62: FaustFloat,
    fHslider73: FaustFloat,
    fHslider64: FaustFloat,
    fHslider63: FaustFloat,
    fHslider75: FaustFloat,
    fHslider74: FaustFloat,
    fHslider78: FaustFloat,
    fRec17485: f32,
    fCheckbox79: FaustFloat,
    fHslider80: FaustFloat,
    fHslider82: FaustFloat,
    fHslider81: FaustFloat,
    iRec17112: [i32; 2],
    iRec17112_1: [i32; 2],
    iRec17112_2: [i32; 2],
    iRec17112_3: [i32; 2],
    iRec17112_4: [i32; 2],
    iRec17112_5: [i32; 2],
    iRec17112_6: [i32; 2],
    fHslider129: FaustFloat,
    fHslider136: FaustFloat,
    fHslider130: FaustFloat,
    fHslider131: FaustFloat,
    fEntry134: FaustFloat,
    fHslider135: FaustFloat,
    fEntry132: FaustFloat,
    fHslider133: FaustFloat,
    fHslider140: FaustFloat,
    fHslider141: FaustFloat,
    fHslider126: FaustFloat,
    fHslider137: FaustFloat,
    fHslider128: FaustFloat,
    fHslider127: FaustFloat,
    fHslider139: FaustFloat,
    fHslider138: FaustFloat,
    fHslider142: FaustFloat,
    fRec17511: f32,
    fCheckbox143: FaustFloat,
    fHslider144: FaustFloat,
    fHslider146: FaustFloat,
    fHslider145: FaustFloat,
    fRec17536: f32,
    iRec17364: [i32; 2],
    iRec17364_1: [i32; 2],
    iRec17364_2: [i32; 2],
    iRec17364_3: [i32; 2],
    iRec17364_4: [i32; 2],
    iRec17364_5: [i32; 2],
    iRec17364_6: [i32; 2],
    fHslider107: FaustFloat,
    fHslider114: FaustFloat,
    fHslider108: FaustFloat,
    fHslider109: FaustFloat,
    fEntry112: FaustFloat,
    fHslider113: FaustFloat,
    fEntry110: FaustFloat,
    fHslider111: FaustFloat,
    fHslider118: FaustFloat,
    fHslider119: FaustFloat,
    fHslider104: FaustFloat,
    fHslider115: FaustFloat,
    fHslider106: FaustFloat,
    fHslider105: FaustFloat,
    fHslider117: FaustFloat,
    fHslider116: FaustFloat,
    fHslider120: FaustFloat,
    fRec17528: f32,
    fCheckbox121: FaustFloat,
    fHslider122: FaustFloat,
    fHslider124: FaustFloat,
    fHslider123: FaustFloat,
    fHslider125: FaustFloat,
}

#[allow(non_snake_case, dead_code, unused_variables, unused_mut, unused_parens, clippy::all)]
impl Dx7Piano {
    pub fn new() -> Dx7Piano {
        Dx7Piano {
            fSampleRate: 0,
            fVec12: [0.0f32; 2],
            iRec15971: [0; 2],
            iRec15971_1: [0; 2],
            iRec15971_2: [0; 2],
            iRec15971_3: [0; 2],
            iRec15971_4: [0; 2],
            iRec15971_5: [0; 2],
            iRec15971_6: [0; 2],
            fButton19: 0.0 as FaustFloat,
            fHslider44: 0.0 as FaustFloat,
            fHslider51: 0.0 as FaustFloat,
            fHslider13: 0.0 as FaustFloat,
            fHslider45: 0.0 as FaustFloat,
            fHslider6: 0.0 as FaustFloat,
            fHslider5: 0.0 as FaustFloat,
            fHslider46: 0.0 as FaustFloat,
            fEntry49: 0.0 as FaustFloat,
            fHslider50: 0.0 as FaustFloat,
            fEntry47: 0.0 as FaustFloat,
            fHslider48: 0.0 as FaustFloat,
            fConst1: 0.0f32,
            fHslider55: 0.0 as FaustFloat,
            fHslider56: 0.0 as FaustFloat,
            fHslider41: 0.0 as FaustFloat,
            fHslider52: 0.0 as FaustFloat,
            fHslider43: 0.0 as FaustFloat,
            fHslider42: 0.0 as FaustFloat,
            fHslider54: 0.0 as FaustFloat,
            fHslider53: 0.0 as FaustFloat,
            fHslider57: 0.0 as FaustFloat,
            fHslider21: 0.0 as FaustFloat,
            fRec16024: [0.0f32; 2],
            fRec16024_1: [0.0f32; 2],
            fRec16024_2: [0.0f32; 2],
            fRec16024_3: [0.0f32; 2],
            iRec16024_4: [0; 2],
            fRec16024_5: [0.0f32; 2],
            fRec16024_6: [0.0f32; 2],
            fConst2: 0.0f32,
            fHslider23: 0.0 as FaustFloat,
            fHslider22: 0.0 as FaustFloat,
            fHslider24: 0.0 as FaustFloat,
            fConst3: 0.0f32,
            fEntry25: 0.0 as FaustFloat,
            fRec17426: 0.0f32,
            fHslider26: 0.0 as FaustFloat,
            fCheckbox58: 0.0 as FaustFloat,
            fHslider59: 0.0 as FaustFloat,
            fHslider61: 0.0 as FaustFloat,
            fHslider60: 0.0 as FaustFloat,
            fRec16090: [0.0f32; 2],
            iRec16090_1: [0; 2],
            iRec16090_2: [0; 2],
            iRec16090_3: [0; 2],
            fRec16090_4: [0.0f32; 2],
            iRec16090_5: [0; 2],
            fHslider31: 0.0 as FaustFloat,
            fHslider34: 0.0 as FaustFloat,
            fConst4: 0.0f32,
            fHslider35: 0.0 as FaustFloat,
            fHslider38: 0.0 as FaustFloat,
            fHslider33: 0.0 as FaustFloat,
            fHslider32: 0.0 as FaustFloat,
            fHslider37: 0.0 as FaustFloat,
            fHslider36: 0.0 as FaustFloat,
            fHslider39: 0.0 as FaustFloat,
            fHslider40: 0.0 as FaustFloat,
            iRec16339: [0; 2],
            iRec16339_1: [0; 2],
            iRec16339_2: [0; 2],
            iRec16339_3: [0; 2],
            iRec16339_4: [0; 2],
            iRec16339_5: [0; 2],
            iRec16339_6: [0; 2],
            fHslider3: 0.0 as FaustFloat,
            fHslider12: 0.0 as FaustFloat,
            fHslider4: 0.0 as FaustFloat,
            fHslider7: 0.0 as FaustFloat,
            fEntry10: 0.0 as FaustFloat,
            fHslider11: 0.0 as FaustFloat,
            fEntry8: 0.0 as FaustFloat,
            fHslider9: 0.0 as FaustFloat,
            fHslider17: 0.0 as FaustFloat,
            fHslider18: 0.0 as FaustFloat,
            fHslider0: 0.0 as FaustFloat,
            fHslider14: 0.0 as FaustFloat,
            fHslider2: 0.0 as FaustFloat,
            fHslider1: 0.0 as FaustFloat,
            fHslider16: 0.0 as FaustFloat,
            fHslider15: 0.0 as FaustFloat,
            fHslider20: 0.0 as FaustFloat,
            fRec17443: 0.0f32,
            fCheckbox27: 0.0 as FaustFloat,
            fHslider28: 0.0 as FaustFloat,
            fHslider30: 0.0 as FaustFloat,
            fHslider29: 0.0 as FaustFloat,
            iRec16599: [0; 2],
            iRec16599_1: [0; 2],
            iRec16599_2: [0; 2],
            iRec16599_3: [0; 2],
            iRec16599_4: [0; 2],
            iRec16599_5: [0; 2],
            iRec16599_6: [0; 2],
            fHslider86: 0.0 as FaustFloat,
            fHslider93: 0.0 as FaustFloat,
            fHslider87: 0.0 as FaustFloat,
            fHslider88: 0.0 as FaustFloat,
            fEntry91: 0.0 as FaustFloat,
            fHslider92: 0.0 as FaustFloat,
            fEntry89: 0.0 as FaustFloat,
            fHslider90: 0.0 as FaustFloat,
            fHslider97: 0.0 as FaustFloat,
            fHslider98: 0.0 as FaustFloat,
            fHslider83: 0.0 as FaustFloat,
            fHslider94: 0.0 as FaustFloat,
            fHslider85: 0.0 as FaustFloat,
            fHslider84: 0.0 as FaustFloat,
            fHslider96: 0.0 as FaustFloat,
            fHslider95: 0.0 as FaustFloat,
            fHslider99: 0.0 as FaustFloat,
            fRec17468: 0.0f32,
            fCheckbox100: 0.0 as FaustFloat,
            fHslider101: 0.0 as FaustFloat,
            fHslider103: 0.0 as FaustFloat,
            fHslider102: 0.0 as FaustFloat,
            iRec16851: [0; 2],
            iRec16851_1: [0; 2],
            iRec16851_2: [0; 2],
            iRec16851_3: [0; 2],
            iRec16851_4: [0; 2],
            iRec16851_5: [0; 2],
            iRec16851_6: [0; 2],
            fHslider65: 0.0 as FaustFloat,
            fHslider72: 0.0 as FaustFloat,
            fHslider66: 0.0 as FaustFloat,
            fHslider67: 0.0 as FaustFloat,
            fEntry70: 0.0 as FaustFloat,
            fHslider71: 0.0 as FaustFloat,
            fEntry68: 0.0 as FaustFloat,
            fHslider69: 0.0 as FaustFloat,
            fHslider76: 0.0 as FaustFloat,
            fHslider77: 0.0 as FaustFloat,
            fHslider62: 0.0 as FaustFloat,
            fHslider73: 0.0 as FaustFloat,
            fHslider64: 0.0 as FaustFloat,
            fHslider63: 0.0 as FaustFloat,
            fHslider75: 0.0 as FaustFloat,
            fHslider74: 0.0 as FaustFloat,
            fHslider78: 0.0 as FaustFloat,
            fRec17485: 0.0f32,
            fCheckbox79: 0.0 as FaustFloat,
            fHslider80: 0.0 as FaustFloat,
            fHslider82: 0.0 as FaustFloat,
            fHslider81: 0.0 as FaustFloat,
            iRec17112: [0; 2],
            iRec17112_1: [0; 2],
            iRec17112_2: [0; 2],
            iRec17112_3: [0; 2],
            iRec17112_4: [0; 2],
            iRec17112_5: [0; 2],
            iRec17112_6: [0; 2],
            fHslider129: 0.0 as FaustFloat,
            fHslider136: 0.0 as FaustFloat,
            fHslider130: 0.0 as FaustFloat,
            fHslider131: 0.0 as FaustFloat,
            fEntry134: 0.0 as FaustFloat,
            fHslider135: 0.0 as FaustFloat,
            fEntry132: 0.0 as FaustFloat,
            fHslider133: 0.0 as FaustFloat,
            fHslider140: 0.0 as FaustFloat,
            fHslider141: 0.0 as FaustFloat,
            fHslider126: 0.0 as FaustFloat,
            fHslider137: 0.0 as FaustFloat,
            fHslider128: 0.0 as FaustFloat,
            fHslider127: 0.0 as FaustFloat,
            fHslider139: 0.0 as FaustFloat,
            fHslider138: 0.0 as FaustFloat,
            fHslider142: 0.0 as FaustFloat,
            fRec17511: 0.0f32,
            fCheckbox143: 0.0 as FaustFloat,
            fHslider144: 0.0 as FaustFloat,
            fHslider146: 0.0 as FaustFloat,
            fHslider145: 0.0 as FaustFloat,
            fRec17536: 0.0f32,
            iRec17364: [0; 2],
            iRec17364_1: [0; 2],
            iRec17364_2: [0; 2],
            iRec17364_3: [0; 2],
            iRec17364_4: [0; 2],
            iRec17364_5: [0; 2],
            iRec17364_6: [0; 2],
            fHslider107: 0.0 as FaustFloat,
            fHslider114: 0.0 as FaustFloat,
            fHslider108: 0.0 as FaustFloat,
            fHslider109: 0.0 as FaustFloat,
            fEntry112: 0.0 as FaustFloat,
            fHslider113: 0.0 as FaustFloat,
            fEntry110: 0.0 as FaustFloat,
            fHslider111: 0.0 as FaustFloat,
            fHslider118: 0.0 as FaustFloat,
            fHslider119: 0.0 as FaustFloat,
            fHslider104: 0.0 as FaustFloat,
            fHslider115: 0.0 as FaustFloat,
            fHslider106: 0.0 as FaustFloat,
            fHslider105: 0.0 as FaustFloat,
            fHslider117: 0.0 as FaustFloat,
            fHslider116: 0.0 as FaustFloat,
            fHslider120: 0.0 as FaustFloat,
            fRec17528: 0.0f32,
            fCheckbox121: 0.0 as FaustFloat,
            fHslider122: 0.0 as FaustFloat,
            fHslider124: 0.0 as FaustFloat,
            fHslider123: 0.0 as FaustFloat,
            fHslider125: 0.0 as FaustFloat,
        }
    }

    pub fn metadata(&self, m: &mut dyn Meta) {
    }

    pub fn get_sample_rate(&self) -> i32 {
        self.fSampleRate
    }

    pub fn get_num_inputs(&self) -> i32 {
        0
    }

    pub fn get_num_outputs(&self) -> i32 {
        2
    }

    pub fn class_init(sample_rate: i32) {
    }

    pub fn instance_constants(&mut self, sample_rate: i32) {
        self.fSampleRate = sample_rate;
        let mut fConst0: f32 = f32::min(192000.0f32, f32::max(1.0f32, ((self.fSampleRate) as f32)));
        self.fConst1 = (44100.0f32 / fConst0);
        self.fConst2 = (1.0f32 / fConst0);
        self.fConst3 = (0.005865102633833885f32 / fConst0);
        self.fConst4 = (1.502347469329834f32 / fConst0);
    }

    pub fn instance_reset_user_interface(&mut self) {
        self.fButton19 = ((0.0f32) as FaustFloat);
        self.fHslider44 = ((0.0f32) as FaustFloat);
        self.fHslider51 = ((0.0f32) as FaustFloat);
        self.fHslider13 = ((0.800000011920929f32) as FaustFloat);
        self.fHslider45 = ((99.0f32) as FaustFloat);
        self.fHslider6 = ((0.0f32) as FaustFloat);
        self.fHslider5 = ((400.0f32) as FaustFloat);
        self.fHslider46 = ((0.0f32) as FaustFloat);
        self.fEntry49 = ((0.0f32) as FaustFloat);
        self.fHslider50 = ((0.0f32) as FaustFloat);
        self.fEntry47 = ((0.0f32) as FaustFloat);
        self.fHslider48 = ((0.0f32) as FaustFloat);
        self.fHslider55 = ((99.0f32) as FaustFloat);
        self.fHslider56 = ((0.0f32) as FaustFloat);
        self.fHslider41 = ((99.0f32) as FaustFloat);
        self.fHslider52 = ((99.0f32) as FaustFloat);
        self.fHslider43 = ((99.0f32) as FaustFloat);
        self.fHslider42 = ((99.0f32) as FaustFloat);
        self.fHslider54 = ((99.0f32) as FaustFloat);
        self.fHslider53 = ((99.0f32) as FaustFloat);
        self.fHslider57 = ((0.0f32) as FaustFloat);
        self.fHslider21 = ((0.0f32) as FaustFloat);
        self.fHslider23 = ((35.0f32) as FaustFloat);
        self.fHslider22 = ((1.0f32) as FaustFloat);
        self.fHslider24 = ((0.0f32) as FaustFloat);
        self.fEntry25 = ((0.0f32) as FaustFloat);
        self.fHslider26 = ((1.0f32) as FaustFloat);
        self.fCheckbox58 = ((0.0f32) as FaustFloat);
        self.fHslider59 = ((0.0f32) as FaustFloat);
        self.fHslider61 = ((0.0f32) as FaustFloat);
        self.fHslider60 = ((1.0f32) as FaustFloat);
        self.fHslider31 = ((50.0f32) as FaustFloat);
        self.fHslider34 = ((50.0f32) as FaustFloat);
        self.fHslider35 = ((99.0f32) as FaustFloat);
        self.fHslider38 = ((99.0f32) as FaustFloat);
        self.fHslider33 = ((50.0f32) as FaustFloat);
        self.fHslider32 = ((50.0f32) as FaustFloat);
        self.fHslider37 = ((99.0f32) as FaustFloat);
        self.fHslider36 = ((99.0f32) as FaustFloat);
        self.fHslider39 = ((0.0f32) as FaustFloat);
        self.fHslider40 = ((3.0f32) as FaustFloat);
        self.fHslider3 = ((0.0f32) as FaustFloat);
        self.fHslider12 = ((0.0f32) as FaustFloat);
        self.fHslider4 = ((0.0f32) as FaustFloat);
        self.fHslider7 = ((0.0f32) as FaustFloat);
        self.fEntry10 = ((0.0f32) as FaustFloat);
        self.fHslider11 = ((0.0f32) as FaustFloat);
        self.fEntry8 = ((0.0f32) as FaustFloat);
        self.fHslider9 = ((0.0f32) as FaustFloat);
        self.fHslider17 = ((99.0f32) as FaustFloat);
        self.fHslider18 = ((0.0f32) as FaustFloat);
        self.fHslider0 = ((99.0f32) as FaustFloat);
        self.fHslider14 = ((99.0f32) as FaustFloat);
        self.fHslider2 = ((99.0f32) as FaustFloat);
        self.fHslider1 = ((99.0f32) as FaustFloat);
        self.fHslider16 = ((99.0f32) as FaustFloat);
        self.fHslider15 = ((99.0f32) as FaustFloat);
        self.fHslider20 = ((0.0f32) as FaustFloat);
        self.fCheckbox27 = ((0.0f32) as FaustFloat);
        self.fHslider28 = ((0.0f32) as FaustFloat);
        self.fHslider30 = ((0.0f32) as FaustFloat);
        self.fHslider29 = ((1.0f32) as FaustFloat);
        self.fHslider86 = ((0.0f32) as FaustFloat);
        self.fHslider93 = ((0.0f32) as FaustFloat);
        self.fHslider87 = ((0.0f32) as FaustFloat);
        self.fHslider88 = ((0.0f32) as FaustFloat);
        self.fEntry91 = ((0.0f32) as FaustFloat);
        self.fHslider92 = ((0.0f32) as FaustFloat);
        self.fEntry89 = ((0.0f32) as FaustFloat);
        self.fHslider90 = ((0.0f32) as FaustFloat);
        self.fHslider97 = ((99.0f32) as FaustFloat);
        self.fHslider98 = ((0.0f32) as FaustFloat);
        self.fHslider83 = ((99.0f32) as FaustFloat);
        self.fHslider94 = ((99.0f32) as FaustFloat);
        self.fHslider85 = ((99.0f32) as FaustFloat);
        self.fHslider84 = ((99.0f32) as FaustFloat);
        self.fHslider96 = ((99.0f32) as FaustFloat);
        self.fHslider95 = ((99.0f32) as FaustFloat);
        self.fHslider99 = ((0.0f32) as FaustFloat);
        self.fCheckbox100 = ((0.0f32) as FaustFloat);
        self.fHslider101 = ((0.0f32) as FaustFloat);
        self.fHslider103 = ((0.0f32) as FaustFloat);
        self.fHslider102 = ((1.0f32) as FaustFloat);
        self.fHslider65 = ((0.0f32) as FaustFloat);
        self.fHslider72 = ((0.0f32) as FaustFloat);
        self.fHslider66 = ((0.0f32) as FaustFloat);
        self.fHslider67 = ((0.0f32) as FaustFloat);
        self.fEntry70 = ((0.0f32) as FaustFloat);
        self.fHslider71 = ((0.0f32) as FaustFloat);
        self.fEntry68 = ((0.0f32) as FaustFloat);
        self.fHslider69 = ((0.0f32) as FaustFloat);
        self.fHslider76 = ((99.0f32) as FaustFloat);
        self.fHslider77 = ((0.0f32) as FaustFloat);
        self.fHslider62 = ((99.0f32) as FaustFloat);
        self.fHslider73 = ((99.0f32) as FaustFloat);
        self.fHslider64 = ((99.0f32) as FaustFloat);
        self.fHslider63 = ((99.0f32) as FaustFloat);
        self.fHslider75 = ((99.0f32) as FaustFloat);
        self.fHslider74 = ((99.0f32) as FaustFloat);
        self.fHslider78 = ((0.0f32) as FaustFloat);
        self.fCheckbox79 = ((0.0f32) as FaustFloat);
        self.fHslider80 = ((0.0f32) as FaustFloat);
        self.fHslider82 = ((0.0f32) as FaustFloat);
        self.fHslider81 = ((1.0f32) as FaustFloat);
        self.fHslider129 = ((0.0f32) as FaustFloat);
        self.fHslider136 = ((0.0f32) as FaustFloat);
        self.fHslider130 = ((0.0f32) as FaustFloat);
        self.fHslider131 = ((0.0f32) as FaustFloat);
        self.fEntry134 = ((0.0f32) as FaustFloat);
        self.fHslider135 = ((0.0f32) as FaustFloat);
        self.fEntry132 = ((0.0f32) as FaustFloat);
        self.fHslider133 = ((0.0f32) as FaustFloat);
        self.fHslider140 = ((99.0f32) as FaustFloat);
        self.fHslider141 = ((0.0f32) as FaustFloat);
        self.fHslider126 = ((99.0f32) as FaustFloat);
        self.fHslider137 = ((99.0f32) as FaustFloat);
        self.fHslider128 = ((99.0f32) as FaustFloat);
        self.fHslider127 = ((99.0f32) as FaustFloat);
        self.fHslider139 = ((99.0f32) as FaustFloat);
        self.fHslider138 = ((99.0f32) as FaustFloat);
        self.fHslider142 = ((0.0f32) as FaustFloat);
        self.fCheckbox143 = ((0.0f32) as FaustFloat);
        self.fHslider144 = ((0.0f32) as FaustFloat);
        self.fHslider146 = ((0.0f32) as FaustFloat);
        self.fHslider145 = ((1.0f32) as FaustFloat);
        self.fHslider107 = ((0.0f32) as FaustFloat);
        self.fHslider114 = ((0.0f32) as FaustFloat);
        self.fHslider108 = ((0.0f32) as FaustFloat);
        self.fHslider109 = ((0.0f32) as FaustFloat);
        self.fEntry112 = ((0.0f32) as FaustFloat);
        self.fHslider113 = ((0.0f32) as FaustFloat);
        self.fEntry110 = ((0.0f32) as FaustFloat);
        self.fHslider111 = ((0.0f32) as FaustFloat);
        self.fHslider118 = ((99.0f32) as FaustFloat);
        self.fHslider119 = ((0.0f32) as FaustFloat);
        self.fHslider104 = ((99.0f32) as FaustFloat);
        self.fHslider115 = ((99.0f32) as FaustFloat);
        self.fHslider106 = ((99.0f32) as FaustFloat);
        self.fHslider105 = ((99.0f32) as FaustFloat);
        self.fHslider117 = ((99.0f32) as FaustFloat);
        self.fHslider116 = ((99.0f32) as FaustFloat);
        self.fHslider120 = ((0.0f32) as FaustFloat);
        self.fCheckbox121 = ((0.0f32) as FaustFloat);
        self.fHslider122 = ((0.0f32) as FaustFloat);
        self.fHslider124 = ((0.0f32) as FaustFloat);
        self.fHslider123 = ((1.0f32) as FaustFloat);
        self.fHslider125 = ((0.0f32) as FaustFloat);
    }

    pub fn instance_clear(&mut self) {
        for lDelay0 in 0..2i32 {
            self.fVec12[(lDelay0) as usize] = 0.0f32;
        }
        for lRec1 in 0..2i32 {
            self.iRec15971[(lRec1) as usize] = 0i32;
        }
        for lRec2 in 0..2i32 {
            self.iRec15971_1[(lRec2) as usize] = 0i32;
        }
        for lRec3 in 0..2i32 {
            self.iRec15971_2[(lRec3) as usize] = 0i32;
        }
        for lRec4 in 0..2i32 {
            self.iRec15971_3[(lRec4) as usize] = 0i32;
        }
        for lRec5 in 0..2i32 {
            self.iRec15971_4[(lRec5) as usize] = 0i32;
        }
        for lRec6 in 0..2i32 {
            self.iRec15971_5[(lRec6) as usize] = 0i32;
        }
        for lRec7 in 0..2i32 {
            self.iRec15971_6[(lRec7) as usize] = 0i32;
        }
        for lRec15 in 0..2i32 {
            self.fRec16024[(lRec15) as usize] = 0.0f32;
        }
        for lRec16 in 0..2i32 {
            self.fRec16024_1[(lRec16) as usize] = 0.0f32;
        }
        for lRec17 in 0..2i32 {
            self.fRec16024_2[(lRec17) as usize] = 0.0f32;
        }
        for lRec18 in 0..2i32 {
            self.fRec16024_3[(lRec18) as usize] = 0.0f32;
        }
        for lRec19 in 0..2i32 {
            self.iRec16024_4[(lRec19) as usize] = 0i32;
        }
        for lRec20 in 0..2i32 {
            self.fRec16024_5[(lRec20) as usize] = 0.0f32;
        }
        for lRec21 in 0..2i32 {
            self.fRec16024_6[(lRec21) as usize] = 0.0f32;
        }
        self.fRec17426 = 0.0f32;
        for lRec29 in 0..2i32 {
            self.fRec16090[(lRec29) as usize] = 0.0f32;
        }
        for lRec30 in 0..2i32 {
            self.iRec16090_1[(lRec30) as usize] = 0i32;
        }
        for lRec31 in 0..2i32 {
            self.iRec16090_2[(lRec31) as usize] = 0i32;
        }
        for lRec32 in 0..2i32 {
            self.iRec16090_3[(lRec32) as usize] = 0i32;
        }
        for lRec33 in 0..2i32 {
            self.fRec16090_4[(lRec33) as usize] = 0.0f32;
        }
        for lRec34 in 0..2i32 {
            self.iRec16090_5[(lRec34) as usize] = 0i32;
        }
        for lRec41 in 0..2i32 {
            self.iRec16339[(lRec41) as usize] = 0i32;
        }
        for lRec42 in 0..2i32 {
            self.iRec16339_1[(lRec42) as usize] = 0i32;
        }
        for lRec43 in 0..2i32 {
            self.iRec16339_2[(lRec43) as usize] = 0i32;
        }
        for lRec44 in 0..2i32 {
            self.iRec16339_3[(lRec44) as usize] = 0i32;
        }
        for lRec45 in 0..2i32 {
            self.iRec16339_4[(lRec45) as usize] = 0i32;
        }
        for lRec46 in 0..2i32 {
            self.iRec16339_5[(lRec46) as usize] = 0i32;
        }
        for lRec47 in 0..2i32 {
            self.iRec16339_6[(lRec47) as usize] = 0i32;
        }
        self.fRec17443 = 0.0f32;
        for lRec55 in 0..2i32 {
            self.iRec16599[(lRec55) as usize] = 0i32;
        }
        for lRec56 in 0..2i32 {
            self.iRec16599_1[(lRec56) as usize] = 0i32;
        }
        for lRec57 in 0..2i32 {
            self.iRec16599_2[(lRec57) as usize] = 0i32;
        }
        for lRec58 in 0..2i32 {
            self.iRec16599_3[(lRec58) as usize] = 0i32;
        }
        for lRec59 in 0..2i32 {
            self.iRec16599_4[(lRec59) as usize] = 0i32;
        }
        for lRec60 in 0..2i32 {
            self.iRec16599_5[(lRec60) as usize] = 0i32;
        }
        for lRec61 in 0..2i32 {
            self.iRec16599_6[(lRec61) as usize] = 0i32;
        }
        self.fRec17468 = 0.0f32;
        for lRec69 in 0..2i32 {
            self.iRec16851[(lRec69) as usize] = 0i32;
        }
        for lRec70 in 0..2i32 {
            self.iRec16851_1[(lRec70) as usize] = 0i32;
        }
        for lRec71 in 0..2i32 {
            self.iRec16851_2[(lRec71) as usize] = 0i32;
        }
        for lRec72 in 0..2i32 {
            self.iRec16851_3[(lRec72) as usize] = 0i32;
        }
        for lRec73 in 0..2i32 {
            self.iRec16851_4[(lRec73) as usize] = 0i32;
        }
        for lRec74 in 0..2i32 {
            self.iRec16851_5[(lRec74) as usize] = 0i32;
        }
        for lRec75 in 0..2i32 {
            self.iRec16851_6[(lRec75) as usize] = 0i32;
        }
        self.fRec17485 = 0.0f32;
        for lRec83 in 0..2i32 {
            self.iRec17112[(lRec83) as usize] = 0i32;
        }
        for lRec84 in 0..2i32 {
            self.iRec17112_1[(lRec84) as usize] = 0i32;
        }
        for lRec85 in 0..2i32 {
            self.iRec17112_2[(lRec85) as usize] = 0i32;
        }
        for lRec86 in 0..2i32 {
            self.iRec17112_3[(lRec86) as usize] = 0i32;
        }
        for lRec87 in 0..2i32 {
            self.iRec17112_4[(lRec87) as usize] = 0i32;
        }
        for lRec88 in 0..2i32 {
            self.iRec17112_5[(lRec88) as usize] = 0i32;
        }
        for lRec89 in 0..2i32 {
            self.iRec17112_6[(lRec89) as usize] = 0i32;
        }
        self.fRec17511 = 0.0f32;
        self.fRec17536 = 0.0f32;
        for lRec97 in 0..2i32 {
            self.iRec17364[(lRec97) as usize] = 0i32;
        }
        for lRec98 in 0..2i32 {
            self.iRec17364_1[(lRec98) as usize] = 0i32;
        }
        for lRec99 in 0..2i32 {
            self.iRec17364_2[(lRec99) as usize] = 0i32;
        }
        for lRec100 in 0..2i32 {
            self.iRec17364_3[(lRec100) as usize] = 0i32;
        }
        for lRec101 in 0..2i32 {
            self.iRec17364_4[(lRec101) as usize] = 0i32;
        }
        for lRec102 in 0..2i32 {
            self.iRec17364_5[(lRec102) as usize] = 0i32;
        }
        for lRec103 in 0..2i32 {
            self.iRec17364_6[(lRec103) as usize] = 0i32;
        }
        self.fRec17528 = 0.0f32;
    }

    pub fn instance_init(&mut self, sample_rate: i32) {
        self.instance_constants(sample_rate);
        self.instance_reset_user_interface();
        self.instance_clear();
    }

    pub fn init(&mut self, sample_rate: i32) {
        Self::class_init(sample_rate);
        self.instance_init(sample_rate);
    }

    pub fn build_user_interface(&mut self, ui_interface: &mut dyn UI<FaustFloat>) {
        ui_interface.open_horizontal_box("DX7");
        ui_interface.open_vertical_box("Global");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Main");
        ui_interface.declare(Some(&mut self.fHslider125), "0", "");
        ui_interface.declare(Some(&mut self.fHslider125), "style", "knob");
        ui_interface.add_horizontal_slider("Feedback", &mut self.fHslider125, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider6), "1", "");
        ui_interface.declare(Some(&mut self.fHslider6), "style", "knob");
        ui_interface.add_horizontal_slider("Transpose", &mut self.fHslider6, 0.0 as FaustFloat, -24.0 as FaustFloat, 24.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider26), "2", "");
        ui_interface.declare(Some(&mut self.fHslider26), "style", "knob");
        ui_interface.add_horizontal_slider("Osc Key Sync", &mut self.fHslider26, 1.0 as FaustFloat, 0.0 as FaustFloat, 1.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Pitch EG Levels");
        ui_interface.declare(Some(&mut self.fHslider31), "0", "");
        ui_interface.declare(Some(&mut self.fHslider31), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider31, 50.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider32), "1", "");
        ui_interface.declare(Some(&mut self.fHslider32), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider32, 50.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider33), "2", "");
        ui_interface.declare(Some(&mut self.fHslider33), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider33, 50.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider34), "3", "");
        ui_interface.declare(Some(&mut self.fHslider34), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider34, 50.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Pitch EG Rates");
        ui_interface.declare(Some(&mut self.fHslider35), "0", "");
        ui_interface.declare(Some(&mut self.fHslider35), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider35, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider36), "1", "");
        ui_interface.declare(Some(&mut self.fHslider36), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider36, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider37), "2", "");
        ui_interface.declare(Some(&mut self.fHslider37), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider37, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider38), "3", "");
        ui_interface.declare(Some(&mut self.fHslider38), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider38, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("LFO");
        ui_interface.declare(Some(&mut self.fEntry25), "0", "");
        ui_interface.declare(Some(&mut self.fEntry25), "style", "menu{'Triangle':0;'Saw Down':1;'Saw Up':2;'Square':3;'Sine':4;'Sample & Hold':5}");
        ui_interface.add_num_entry("Wave", &mut self.fEntry25, 0.0 as FaustFloat, 0.0 as FaustFloat, 5.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider23), "1", "");
        ui_interface.declare(Some(&mut self.fHslider23), "style", "knob");
        ui_interface.add_horizontal_slider("Speed", &mut self.fHslider23, 35.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider24), "2", "");
        ui_interface.declare(Some(&mut self.fHslider24), "style", "knob");
        ui_interface.add_horizontal_slider("Delay", &mut self.fHslider24, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider39), "3", "");
        ui_interface.declare(Some(&mut self.fHslider39), "style", "knob");
        ui_interface.add_horizontal_slider("PMD", &mut self.fHslider39, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider21), "4", "");
        ui_interface.declare(Some(&mut self.fHslider21), "style", "knob");
        ui_interface.add_horizontal_slider("AMD", &mut self.fHslider21, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider22), "5", "");
        ui_interface.declare(Some(&mut self.fHslider22), "style", "knob");
        ui_interface.add_horizontal_slider("Sync", &mut self.fHslider22, 1.0 as FaustFloat, 0.0 as FaustFloat, 1.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider40), "6", "");
        ui_interface.declare(Some(&mut self.fHslider40), "style", "knob");
        ui_interface.add_horizontal_slider("P Mod Sens", &mut self.fHslider40, 3.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "0", "");
        ui_interface.open_vertical_box("Operator 1");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider59), "0", "");
        ui_interface.declare(Some(&mut self.fHslider59), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider59, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider60), "1", "");
        ui_interface.declare(Some(&mut self.fHslider60), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider60, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider61), "2", "");
        ui_interface.declare(Some(&mut self.fHslider61), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider61, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox58), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox58), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox58);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider41), "0", "");
        ui_interface.declare(Some(&mut self.fHslider41), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider41, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider42), "1", "");
        ui_interface.declare(Some(&mut self.fHslider42), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider42, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider43), "2", "");
        ui_interface.declare(Some(&mut self.fHslider43), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider43, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider44), "3", "");
        ui_interface.declare(Some(&mut self.fHslider44), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider44, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider52), "0", "");
        ui_interface.declare(Some(&mut self.fHslider52), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider52, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider53), "1", "");
        ui_interface.declare(Some(&mut self.fHslider53), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider53, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider54), "2", "");
        ui_interface.declare(Some(&mut self.fHslider54), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider54, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider55), "3", "");
        ui_interface.declare(Some(&mut self.fHslider55), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider55, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider45), "0", "");
        ui_interface.declare(Some(&mut self.fHslider45), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider45, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider51), "1", "");
        ui_interface.declare(Some(&mut self.fHslider51), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider51, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider57), "2", "");
        ui_interface.declare(Some(&mut self.fHslider57), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider57, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider56), "3", "");
        ui_interface.declare(Some(&mut self.fHslider56), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider56, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider46), "0", "");
        ui_interface.declare(Some(&mut self.fHslider46), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider46, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider48), "1", "");
        ui_interface.declare(Some(&mut self.fHslider48), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider48, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider50), "2", "");
        ui_interface.declare(Some(&mut self.fHslider50), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider50, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry47), "3", "");
        ui_interface.declare(Some(&mut self.fEntry47), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry47, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry49), "4", "");
        ui_interface.declare(Some(&mut self.fEntry49), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry49, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Operator 2");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider28), "0", "");
        ui_interface.declare(Some(&mut self.fHslider28), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider28, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider29), "1", "");
        ui_interface.declare(Some(&mut self.fHslider29), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider29, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider30), "2", "");
        ui_interface.declare(Some(&mut self.fHslider30), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider30, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox27), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox27), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox27);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider0), "0", "");
        ui_interface.declare(Some(&mut self.fHslider0), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider0, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider1), "1", "");
        ui_interface.declare(Some(&mut self.fHslider1), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider1, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider2), "2", "");
        ui_interface.declare(Some(&mut self.fHslider2), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider2, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider3), "3", "");
        ui_interface.declare(Some(&mut self.fHslider3), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider3, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider14), "0", "");
        ui_interface.declare(Some(&mut self.fHslider14), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider14, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider15), "1", "");
        ui_interface.declare(Some(&mut self.fHslider15), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider15, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider16), "2", "");
        ui_interface.declare(Some(&mut self.fHslider16), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider16, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider17), "3", "");
        ui_interface.declare(Some(&mut self.fHslider17), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider17, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider4), "0", "");
        ui_interface.declare(Some(&mut self.fHslider4), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider4, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider12), "1", "");
        ui_interface.declare(Some(&mut self.fHslider12), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider12, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider20), "2", "");
        ui_interface.declare(Some(&mut self.fHslider20), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider20, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider18), "3", "");
        ui_interface.declare(Some(&mut self.fHslider18), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider18, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider7), "0", "");
        ui_interface.declare(Some(&mut self.fHslider7), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider7, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider9), "1", "");
        ui_interface.declare(Some(&mut self.fHslider9), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider9, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider11), "2", "");
        ui_interface.declare(Some(&mut self.fHslider11), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider11, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry8), "3", "");
        ui_interface.declare(Some(&mut self.fEntry8), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry8, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry10), "4", "");
        ui_interface.declare(Some(&mut self.fEntry10), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry10, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_vertical_box("Operator 3");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider101), "0", "");
        ui_interface.declare(Some(&mut self.fHslider101), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider101, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider102), "1", "");
        ui_interface.declare(Some(&mut self.fHslider102), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider102, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider103), "2", "");
        ui_interface.declare(Some(&mut self.fHslider103), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider103, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox100), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox100), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox100);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider83), "0", "");
        ui_interface.declare(Some(&mut self.fHslider83), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider83, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider84), "1", "");
        ui_interface.declare(Some(&mut self.fHslider84), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider84, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider85), "2", "");
        ui_interface.declare(Some(&mut self.fHslider85), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider85, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider86), "3", "");
        ui_interface.declare(Some(&mut self.fHslider86), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider86, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider94), "0", "");
        ui_interface.declare(Some(&mut self.fHslider94), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider94, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider95), "1", "");
        ui_interface.declare(Some(&mut self.fHslider95), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider95, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider96), "2", "");
        ui_interface.declare(Some(&mut self.fHslider96), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider96, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider97), "3", "");
        ui_interface.declare(Some(&mut self.fHslider97), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider97, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider87), "0", "");
        ui_interface.declare(Some(&mut self.fHslider87), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider87, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider93), "1", "");
        ui_interface.declare(Some(&mut self.fHslider93), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider93, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider99), "2", "");
        ui_interface.declare(Some(&mut self.fHslider99), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider99, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider98), "3", "");
        ui_interface.declare(Some(&mut self.fHslider98), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider98, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider88), "0", "");
        ui_interface.declare(Some(&mut self.fHslider88), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider88, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider90), "1", "");
        ui_interface.declare(Some(&mut self.fHslider90), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider90, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider92), "2", "");
        ui_interface.declare(Some(&mut self.fHslider92), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider92, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry89), "3", "");
        ui_interface.declare(Some(&mut self.fEntry89), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry89, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry91), "4", "");
        ui_interface.declare(Some(&mut self.fEntry91), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry91, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_vertical_box("Operator 4");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider80), "0", "");
        ui_interface.declare(Some(&mut self.fHslider80), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider80, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider81), "1", "");
        ui_interface.declare(Some(&mut self.fHslider81), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider81, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider82), "2", "");
        ui_interface.declare(Some(&mut self.fHslider82), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider82, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox79), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox79), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox79);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider62), "0", "");
        ui_interface.declare(Some(&mut self.fHslider62), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider62, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider63), "1", "");
        ui_interface.declare(Some(&mut self.fHslider63), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider63, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider64), "2", "");
        ui_interface.declare(Some(&mut self.fHslider64), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider64, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider65), "3", "");
        ui_interface.declare(Some(&mut self.fHslider65), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider65, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider73), "0", "");
        ui_interface.declare(Some(&mut self.fHslider73), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider73, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider74), "1", "");
        ui_interface.declare(Some(&mut self.fHslider74), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider74, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider75), "2", "");
        ui_interface.declare(Some(&mut self.fHslider75), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider75, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider76), "3", "");
        ui_interface.declare(Some(&mut self.fHslider76), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider76, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider66), "0", "");
        ui_interface.declare(Some(&mut self.fHslider66), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider66, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider72), "1", "");
        ui_interface.declare(Some(&mut self.fHslider72), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider72, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider78), "2", "");
        ui_interface.declare(Some(&mut self.fHslider78), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider78, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider77), "3", "");
        ui_interface.declare(Some(&mut self.fHslider77), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider77, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider67), "0", "");
        ui_interface.declare(Some(&mut self.fHslider67), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider67, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider69), "1", "");
        ui_interface.declare(Some(&mut self.fHslider69), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider69, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider71), "2", "");
        ui_interface.declare(Some(&mut self.fHslider71), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider71, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry68), "3", "");
        ui_interface.declare(Some(&mut self.fEntry68), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry68, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry70), "4", "");
        ui_interface.declare(Some(&mut self.fEntry70), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry70, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "4", "");
        ui_interface.open_vertical_box("Operator 5");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider144), "0", "");
        ui_interface.declare(Some(&mut self.fHslider144), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider144, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider145), "1", "");
        ui_interface.declare(Some(&mut self.fHslider145), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider145, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider146), "2", "");
        ui_interface.declare(Some(&mut self.fHslider146), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider146, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox143), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox143), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox143);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider126), "0", "");
        ui_interface.declare(Some(&mut self.fHslider126), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider126, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider127), "1", "");
        ui_interface.declare(Some(&mut self.fHslider127), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider127, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider128), "2", "");
        ui_interface.declare(Some(&mut self.fHslider128), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider128, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider129), "3", "");
        ui_interface.declare(Some(&mut self.fHslider129), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider129, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider137), "0", "");
        ui_interface.declare(Some(&mut self.fHslider137), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider137, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider138), "1", "");
        ui_interface.declare(Some(&mut self.fHslider138), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider138, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider139), "2", "");
        ui_interface.declare(Some(&mut self.fHslider139), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider139, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider140), "3", "");
        ui_interface.declare(Some(&mut self.fHslider140), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider140, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider130), "0", "");
        ui_interface.declare(Some(&mut self.fHslider130), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider130, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider136), "1", "");
        ui_interface.declare(Some(&mut self.fHslider136), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider136, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider142), "2", "");
        ui_interface.declare(Some(&mut self.fHslider142), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider142, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider141), "3", "");
        ui_interface.declare(Some(&mut self.fHslider141), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider141, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider131), "0", "");
        ui_interface.declare(Some(&mut self.fHslider131), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider131, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider133), "1", "");
        ui_interface.declare(Some(&mut self.fHslider133), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider133, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider135), "2", "");
        ui_interface.declare(Some(&mut self.fHslider135), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider135, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry132), "3", "");
        ui_interface.declare(Some(&mut self.fEntry132), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry132, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry134), "4", "");
        ui_interface.declare(Some(&mut self.fEntry134), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry134, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "5", "");
        ui_interface.open_vertical_box("Operator 6");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Tone");
        ui_interface.declare(Some(&mut self.fHslider122), "0", "");
        ui_interface.declare(Some(&mut self.fHslider122), "style", "knob");
        ui_interface.add_horizontal_slider("Tune", &mut self.fHslider122, 0.0 as FaustFloat, -7.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider123), "1", "");
        ui_interface.declare(Some(&mut self.fHslider123), "style", "knob");
        ui_interface.add_horizontal_slider("Coarse", &mut self.fHslider123, 1.0 as FaustFloat, 0.0 as FaustFloat, 31.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider124), "2", "");
        ui_interface.declare(Some(&mut self.fHslider124), "style", "knob");
        ui_interface.add_horizontal_slider("Fine", &mut self.fHslider124, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fCheckbox121), "3", "");
        ui_interface.declare(Some(&mut self.fCheckbox121), "style", "knob");
        ui_interface.add_check_button("Freq Mode", &mut self.fCheckbox121);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_vertical_box("Amp Env Generator");
        ui_interface.declare(None, "0", "");
        ui_interface.open_horizontal_box("Levels");
        ui_interface.declare(Some(&mut self.fHslider104), "0", "");
        ui_interface.declare(Some(&mut self.fHslider104), "style", "knob");
        ui_interface.add_horizontal_slider("L1", &mut self.fHslider104, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider105), "1", "");
        ui_interface.declare(Some(&mut self.fHslider105), "style", "knob");
        ui_interface.add_horizontal_slider("L2", &mut self.fHslider105, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider106), "2", "");
        ui_interface.declare(Some(&mut self.fHslider106), "style", "knob");
        ui_interface.add_horizontal_slider("L3", &mut self.fHslider106, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider107), "3", "");
        ui_interface.declare(Some(&mut self.fHslider107), "style", "knob");
        ui_interface.add_horizontal_slider("L4", &mut self.fHslider107, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "1", "");
        ui_interface.open_horizontal_box("Rates");
        ui_interface.declare(Some(&mut self.fHslider115), "0", "");
        ui_interface.declare(Some(&mut self.fHslider115), "style", "knob");
        ui_interface.add_horizontal_slider("R1", &mut self.fHslider115, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider116), "1", "");
        ui_interface.declare(Some(&mut self.fHslider116), "style", "knob");
        ui_interface.add_horizontal_slider("R2", &mut self.fHslider116, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider117), "2", "");
        ui_interface.declare(Some(&mut self.fHslider117), "style", "knob");
        ui_interface.add_horizontal_slider("R3", &mut self.fHslider117, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider118), "3", "");
        ui_interface.declare(Some(&mut self.fHslider118), "style", "knob");
        ui_interface.add_horizontal_slider("R4", &mut self.fHslider118, 99.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(None, "2", "");
        ui_interface.open_horizontal_box("Level");
        ui_interface.declare(Some(&mut self.fHslider108), "0", "");
        ui_interface.declare(Some(&mut self.fHslider108), "style", "knob");
        ui_interface.add_horizontal_slider("Level", &mut self.fHslider108, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider114), "1", "");
        ui_interface.declare(Some(&mut self.fHslider114), "style", "knob");
        ui_interface.add_horizontal_slider("Key Vel", &mut self.fHslider114, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider120), "2", "");
        ui_interface.declare(Some(&mut self.fHslider120), "style", "knob");
        ui_interface.add_horizontal_slider("A Mod Sens", &mut self.fHslider120, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider119), "3", "");
        ui_interface.declare(Some(&mut self.fHslider119), "style", "knob");
        ui_interface.add_horizontal_slider("Rate Scaling", &mut self.fHslider119, 0.0 as FaustFloat, 0.0 as FaustFloat, 7.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.declare(None, "3", "");
        ui_interface.open_horizontal_box("Breakpoint");
        ui_interface.declare(Some(&mut self.fHslider109), "0", "");
        ui_interface.declare(Some(&mut self.fHslider109), "style", "knob");
        ui_interface.add_horizontal_slider("Breakpoint", &mut self.fHslider109, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider111), "1", "");
        ui_interface.declare(Some(&mut self.fHslider111), "style", "knob");
        ui_interface.add_horizontal_slider("L Depth", &mut self.fHslider111, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider113), "2", "");
        ui_interface.declare(Some(&mut self.fHslider113), "style", "knob");
        ui_interface.add_horizontal_slider("R Depth", &mut self.fHslider113, 0.0 as FaustFloat, 0.0 as FaustFloat, 99.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry110), "3", "");
        ui_interface.declare(Some(&mut self.fEntry110), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("L Curve", &mut self.fEntry110, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.declare(Some(&mut self.fEntry112), "4", "");
        ui_interface.declare(Some(&mut self.fEntry112), "style", "menu{'-LIN':0;'-EXP':1;'+EXP':2;'+LIN':3}");
        ui_interface.add_num_entry("R Curve", &mut self.fEntry112, 0.0 as FaustFloat, 0.0 as FaustFloat, 3.0 as FaustFloat, 1.0 as FaustFloat);
        ui_interface.close_box();
        ui_interface.close_box();
        ui_interface.declare(Some(&mut self.fHslider5), "hidden", "1");
        ui_interface.add_horizontal_slider("freq", &mut self.fHslider5, 400.0 as FaustFloat, 50.0 as FaustFloat, 1000.0 as FaustFloat, 0.01 as FaustFloat);
        ui_interface.declare(Some(&mut self.fHslider13), "hidden", "1");
        ui_interface.add_horizontal_slider("gain", &mut self.fHslider13, 0.8 as FaustFloat, 0.0 as FaustFloat, 1.0 as FaustFloat, 0.01 as FaustFloat);
        ui_interface.declare(Some(&mut self.fButton19), "hidden", "1");
        ui_interface.add_button("gate", &mut self.fButton19);
        ui_interface.close_box();
    }

    pub fn compute(&mut self, count: i32, inputs: &[&[FaustFloat]], outputs: &mut [&mut [FaustFloat]]) {
        // signal_fir_fastlane_step2a: executable base slice
        // io: inputs=0 outputs=2
        // signals: 2
        let mut fSlow0: f32 = ((self.fButton19) as f32);
        let mut fSlow1: f32 = f32::round(((self.fHslider44) as f32));
        let mut iSlow0: i32 = (((fSlow1 >= 20.0f32)) as i32);
        let mut fSlow2: f32 = (fSlow1 + 28.0f32);
        let mut iSlow1: i32 = ((f32::round(fSlow1)) as i32);
        let mut fSlow3: f32 = f32::round(((self.fHslider51) as f32));
        let mut iSlow2: i32 = ((f32::round((((((f32::max(0.0f32, f32::min(127.0f32, (127.0f32 * ((self.fHslider13) as f32))))) as i32)).wrapping_shr((1i32) as u32)) as f32))) as i32);
        let mut fSlow4: f32 = f32::round(((self.fHslider45) as f32));
        let mut iSlow3: i32 = (((fSlow4 >= 20.0f32)) as i32);
        let mut fSlow5: f32 = (fSlow4 + 28.0f32);
        let mut iSlow4: i32 = ((f32::round(fSlow4)) as i32);
        let mut fSlow6: f32 = f32::powf(2.0f32, (0.0833333358168602f32 * (f32::round(((self.fHslider6) as f32)) + (17.312339782714844f32 * f32::ln((0.0022727272007614374f32 * ((self.fHslider5) as f32)))))));
        let mut fSlow7: f32 = f32::round(((17.312339782714844f32 * f32::ln(fSlow6)) + 69.0f32));
        let mut fSlow8: f32 = f32::round(((self.fHslider46) as f32));
        let mut iSlow5: i32 = ((((fSlow7 - (fSlow8 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow9: f32 = f32::round(((self.fEntry49) as f32));
        let mut iSlow6: i32 = (((fSlow9 < 2.0f32)) as i32);
        let mut iSlow7: i32 = ((((fSlow9 == 0.0f32)) as i32) | (((fSlow9 == 3.0f32)) as i32));
        let mut fSlow10: f32 = f32::round(((self.fHslider50) as f32));
        let mut fSlow11: f32 = (fSlow7 - (fSlow8 + 16.0f32));
        let mut iSlow8: i32 = (((((109.66666412353516f32 * fSlow10) * fSlow11)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow12: f32 = (329.0f32 * fSlow10);
        let mut iSlow9: i32 = ((f32::round((0.3333333432674408f32 * fSlow11))) as i32);
        let mut fSlow13: f32 = f32::round(((self.fEntry47) as f32));
        let mut iSlow10: i32 = (((fSlow13 < 2.0f32)) as i32);
        let mut iSlow11: i32 = ((((fSlow13 == 0.0f32)) as i32) | (((fSlow13 == 3.0f32)) as i32));
        let mut fSlow14: f32 = f32::round(((self.fHslider48) as f32));
        let mut fSlow15: f32 = (18.0f32 - fSlow7);
        let mut fSlow16: f32 = (fSlow8 + fSlow15);
        let mut iSlow12: i32 = (((((109.66666412353516f32 * fSlow14) * fSlow16)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow17: f32 = (329.0f32 * fSlow14);
        let mut iSlow13: i32 = ((f32::round((0.3333333432674408f32 * fSlow16))) as i32);
        let mut fSlow18: f32 = f32::round(((self.fHslider55) as f32));
        let mut fSlow19: f32 = f32::round(((self.fHslider56) as f32));
        let mut iSlow14: i32 = i32::min(31i32, i32::max(0i32, ((((0.3333333432674408f32 * fSlow7)) as i32)).wrapping_sub(7i32)));
        let mut iSlow15: i32 = (iSlow14 & 7i32);
        let mut iSlow16: i32 = (((iSlow15 == 3i32)) as i32);
        let mut fSlow20: f32 = ((iSlow14) as f32);
        let mut iSlow17: i32 = ((((fSlow19 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow18: i32 = (((iSlow15 > 0i32)) as i32);
        let mut iSlow19: i32 = (((iSlow15 < 4i32)) as i32);
        let mut iSlow20: i32 = (if ((((((fSlow19 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow17).wrapping_sub(1i32) } else { (if (((((((fSlow19 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow17).wrapping_add(1i32) } else { iSlow17 }) });
        let mut fSlow21: f32 = ((iSlow20) as f32);
        let mut fSlow22: f32 = f32::min((fSlow18 + fSlow21), 99.0f32);
        let mut iSlow21: i32 = (((fSlow22 < 77.0f32)) as i32);
        let mut iSlow22: i32 = ((f32::round(fSlow22)) as i32);
        let mut fSlow23: f32 = (20.0f32 * (99.0f32 - fSlow22));
        let mut fSlow24: f32 = f32::round(((self.fHslider41) as f32));
        let mut iSlow23: i32 = (((fSlow24 >= 20.0f32)) as i32);
        let mut fSlow25: f32 = (fSlow24 + 28.0f32);
        let mut iSlow24: i32 = ((f32::round(fSlow24)) as i32);
        let mut iSlow25: i32 = (((fSlow24 == 0.0f32)) as i32);
        let mut fSlow26: f32 = f32::round(((self.fHslider52) as f32));
        let mut fSlow27: f32 = f32::min((fSlow26 + fSlow21), 99.0f32);
        let mut iSlow26: i32 = (((fSlow27 < 77.0f32)) as i32);
        let mut iSlow27: i32 = (iSlow26 & iSlow25);
        let mut iSlow28: i32 = ((f32::round(fSlow27)) as i32);
        let mut fSlow28: f32 = (20.0f32 * (99.0f32 - fSlow27));
        let mut fSlow29: f32 = f32::round(((self.fHslider43) as f32));
        let mut fSlow30: f32 = f32::round(((self.fHslider42) as f32));
        let mut fSlow31: f32 = f32::round(((self.fHslider54) as f32));
        let mut fSlow32: f32 = f32::round(((self.fHslider53) as f32));
        let mut iSlow29: i32 = i32::min((iSlow20).wrapping_add(((41i32).wrapping_mul(((fSlow18) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow30: i32 = (((self.fConst1 * (((((iSlow29 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow29).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow31: i32 = i32::min((iSlow20).wrapping_add(((41i32).wrapping_mul(((fSlow26) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow32: i32 = (((self.fConst1 * (((((iSlow31 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow31).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow33: i32 = ((f32::round(f32::round(((self.fHslider57) as f32)))) as i32);
        let mut fSlow33: f32 = (2.6972606370634367e-9f32 * f32::round(((self.fHslider21) as f32)));
        let mut fSlow34: f32 = f32::round(((self.fHslider23) as f32));
        let mut fSlow35: f32 = (self.fConst2 * (if ((0.010101010091602802f32 * fSlow34) <= 0.6565660238265991f32) { ((0.15806305408477783f32 * fSlow34) + 0.03647800162434578f32) } else { ((1.1002540588378906f32 * fSlow34) - 61.2059326171875f32) }));
        let mut iSlow34: i32 = ((f32::round(((self.fHslider22) as f32))) as i32);
        let mut fSlow36: f32 = (99.0f32 - f32::round(((self.fHslider24) as f32)));
        let mut iSlow35: i32 = ((((((fSlow36 == 99.0f32)) as i32) >= 1i32)) as i32);
        let mut iSlow36: i32 = ((fSlow36) as i32);
        let mut iSlow37: i32 = (((iSlow36 & 15i32)).wrapping_add(16i32)).wrapping_shl((((iSlow36).wrapping_shr((4i32) as u32)).wrapping_add(1i32)) as u32);
        let mut fSlow37: f32 = (if ((iSlow35) != 0) { 1.0f32 } else { (self.fConst3 * ((iSlow37) as f32)) });
        let mut fSlow38: f32 = (if ((iSlow35) != 0) { 1.0f32 } else { (self.fConst3 * ((i32::max((iSlow37 & 65408i32), 128i32)) as f32)) });
        let mut fSlow39: f32 = f32::round(((self.fEntry25) as f32));
        let mut iSlow38: i32 = (((fSlow39 >= 3.0f32)) as i32);
        let mut iSlow39: i32 = (((fSlow39 >= 5.0f32)) as i32);
        let mut iSlow40: i32 = (((fSlow39 >= 4.0f32)) as i32);
        let mut iSlow41: i32 = (((fSlow39 >= 2.0f32)) as i32);
        let mut iSlow42: i32 = (((fSlow39 >= 1.0f32)) as i32);
        let mut iSlow43: i32 = ((f32::round(((self.fHslider26) as f32))) as i32);
        let mut iSlow44: i32 = ((f32::round(((self.fCheckbox58) as f32))) as i32);
        let mut fSlow40: f32 = f32::round(((self.fHslider59) as f32));
        let mut fSlow41: f32 = f32::round(((self.fHslider61) as f32));
        let mut iSlow45: i32 = ((f32::round(((self.fHslider60) as f32))) as i32);
        let mut fSlow42: f32 = ((if (fSlow40 > 0.0f32) { (13457.0f32 * fSlow40) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow41 + (((100i32).wrapping_mul((iSlow45 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow43: f32 = (((if ((((fSlow41) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow41) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow46: i32 = ((f32::round((((iSlow45 & 31i32)) as f32))) as i32);
        let mut fSlow44: f32 = f32::ln((440.0f32 * fSlow6));
        let mut fSlow45: f32 = f32::exp((-1.0f32 * (0.5713072419166565f32 * fSlow44)));
        let mut fSlow46: f32 = (fSlow44 * (((72267.4453125f32 * fSlow40) * fSlow45) + 24204406.0f32));
        let mut fSlow47: f32 = f32::round(((self.fHslider31) as f32));
        let mut iSlow47: i32 = ((f32::round(fSlow47)) as i32);
        let mut fSlow48: f32 = f32::round(((self.fHslider34) as f32));
        let mut iSlow48: i32 = ((f32::round(fSlow48)) as i32);
        let mut fSlow49: f32 = f32::round(((self.fHslider35) as f32));
        let mut iSlow49: i32 = ((f32::round(fSlow49)) as i32);
        let mut fSlow50: f32 = f32::round(((self.fHslider38) as f32));
        let mut iSlow50: i32 = ((f32::round(fSlow50)) as i32);
        let mut fSlow51: f32 = f32::round(((self.fHslider33) as f32));
        let mut fSlow52: f32 = f32::round(((self.fHslider32) as f32));
        let mut fSlow53: f32 = f32::round(((self.fHslider37) as f32));
        let mut fSlow54: f32 = f32::round(((self.fHslider36) as f32));
        let mut fSlow55: f32 = (7.891414134064689e-5f32 * f32::round(((self.fHslider39) as f32)));
        let mut iSlow51: i32 = ((f32::round(f32::round(((self.fHslider40) as f32)))) as i32);
        let mut fSlow56: f32 = f32::round(((self.fHslider3) as f32));
        let mut iSlow52: i32 = (((fSlow56 >= 20.0f32)) as i32);
        let mut fSlow57: f32 = (fSlow56 + 28.0f32);
        let mut iSlow53: i32 = ((f32::round(fSlow56)) as i32);
        let mut fSlow58: f32 = f32::round(((self.fHslider12) as f32));
        let mut fSlow59: f32 = f32::round(((self.fHslider4) as f32));
        let mut iSlow54: i32 = (((fSlow59 >= 20.0f32)) as i32);
        let mut fSlow60: f32 = (fSlow59 + 28.0f32);
        let mut iSlow55: i32 = ((f32::round(fSlow59)) as i32);
        let mut fSlow61: f32 = f32::round(((self.fHslider7) as f32));
        let mut iSlow56: i32 = ((((fSlow7 - (fSlow61 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow62: f32 = f32::round(((self.fEntry10) as f32));
        let mut iSlow57: i32 = (((fSlow62 < 2.0f32)) as i32);
        let mut iSlow58: i32 = ((((fSlow62 == 0.0f32)) as i32) | (((fSlow62 == 3.0f32)) as i32));
        let mut fSlow63: f32 = f32::round(((self.fHslider11) as f32));
        let mut fSlow64: f32 = (fSlow7 - (fSlow61 + 16.0f32));
        let mut iSlow59: i32 = (((((109.66666412353516f32 * fSlow63) * fSlow64)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow65: f32 = (329.0f32 * fSlow63);
        let mut iSlow60: i32 = ((f32::round((0.3333333432674408f32 * fSlow64))) as i32);
        let mut fSlow66: f32 = f32::round(((self.fEntry8) as f32));
        let mut iSlow61: i32 = (((fSlow66 < 2.0f32)) as i32);
        let mut iSlow62: i32 = ((((fSlow66 == 0.0f32)) as i32) | (((fSlow66 == 3.0f32)) as i32));
        let mut fSlow67: f32 = f32::round(((self.fHslider9) as f32));
        let mut fSlow68: f32 = (fSlow61 + fSlow15);
        let mut iSlow63: i32 = (((((109.66666412353516f32 * fSlow67) * fSlow68)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow69: f32 = (329.0f32 * fSlow67);
        let mut iSlow64: i32 = ((f32::round((0.3333333432674408f32 * fSlow68))) as i32);
        let mut fSlow70: f32 = f32::round(((self.fHslider17) as f32));
        let mut fSlow71: f32 = f32::round(((self.fHslider18) as f32));
        let mut iSlow65: i32 = ((((fSlow71 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow66: i32 = (if ((((((fSlow71 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow65).wrapping_sub(1i32) } else { (if (((((((fSlow71 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow65).wrapping_add(1i32) } else { iSlow65 }) });
        let mut fSlow72: f32 = ((iSlow66) as f32);
        let mut fSlow73: f32 = f32::min((fSlow70 + fSlow72), 99.0f32);
        let mut iSlow67: i32 = (((fSlow73 < 77.0f32)) as i32);
        let mut iSlow68: i32 = ((f32::round(fSlow73)) as i32);
        let mut fSlow74: f32 = (20.0f32 * (99.0f32 - fSlow73));
        let mut fSlow75: f32 = f32::round(((self.fHslider0) as f32));
        let mut iSlow69: i32 = (((fSlow75 >= 20.0f32)) as i32);
        let mut fSlow76: f32 = (fSlow75 + 28.0f32);
        let mut iSlow70: i32 = ((f32::round(fSlow75)) as i32);
        let mut iSlow71: i32 = (((fSlow75 == 0.0f32)) as i32);
        let mut fSlow77: f32 = f32::round(((self.fHslider14) as f32));
        let mut fSlow78: f32 = f32::min((fSlow77 + fSlow72), 99.0f32);
        let mut iSlow72: i32 = (((fSlow78 < 77.0f32)) as i32);
        let mut iSlow73: i32 = (iSlow72 & iSlow71);
        let mut iSlow74: i32 = ((f32::round(fSlow78)) as i32);
        let mut fSlow79: f32 = (20.0f32 * (99.0f32 - fSlow78));
        let mut fSlow80: f32 = f32::round(((self.fHslider2) as f32));
        let mut fSlow81: f32 = f32::round(((self.fHslider1) as f32));
        let mut fSlow82: f32 = f32::round(((self.fHslider16) as f32));
        let mut fSlow83: f32 = f32::round(((self.fHslider15) as f32));
        let mut iSlow75: i32 = i32::min((iSlow66).wrapping_add(((41i32).wrapping_mul(((fSlow70) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow76: i32 = (((self.fConst1 * (((((iSlow75 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow75).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow77: i32 = i32::min((iSlow66).wrapping_add(((41i32).wrapping_mul(((fSlow77) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow78: i32 = (((self.fConst1 * (((((iSlow77 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow77).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow79: i32 = ((f32::round(f32::round(((self.fHslider20) as f32)))) as i32);
        let mut iSlow80: i32 = ((f32::round(((self.fCheckbox27) as f32))) as i32);
        let mut fSlow84: f32 = f32::round(((self.fHslider28) as f32));
        let mut fSlow85: f32 = f32::round(((self.fHslider30) as f32));
        let mut iSlow81: i32 = ((f32::round(((self.fHslider29) as f32))) as i32);
        let mut fSlow86: f32 = ((if (fSlow84 > 0.0f32) { (13457.0f32 * fSlow84) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow85 + (((100i32).wrapping_mul((iSlow81 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow87: f32 = (((if ((((fSlow85) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow85) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow82: i32 = ((f32::round((((iSlow81 & 31i32)) as f32))) as i32);
        let mut fSlow88: f32 = (fSlow44 * (((72267.4453125f32 * fSlow84) * fSlow45) + 24204406.0f32));
        let mut fSlow89: f32 = f32::round(((self.fHslider86) as f32));
        let mut iSlow83: i32 = (((fSlow89 >= 20.0f32)) as i32);
        let mut fSlow90: f32 = (fSlow89 + 28.0f32);
        let mut iSlow84: i32 = ((f32::round(fSlow89)) as i32);
        let mut fSlow91: f32 = f32::round(((self.fHslider93) as f32));
        let mut fSlow92: f32 = f32::round(((self.fHslider87) as f32));
        let mut iSlow85: i32 = (((fSlow92 >= 20.0f32)) as i32);
        let mut fSlow93: f32 = (fSlow92 + 28.0f32);
        let mut iSlow86: i32 = ((f32::round(fSlow92)) as i32);
        let mut fSlow94: f32 = f32::round(((self.fHslider88) as f32));
        let mut iSlow87: i32 = ((((fSlow7 - (fSlow94 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow95: f32 = f32::round(((self.fEntry91) as f32));
        let mut iSlow88: i32 = (((fSlow95 < 2.0f32)) as i32);
        let mut iSlow89: i32 = ((((fSlow95 == 0.0f32)) as i32) | (((fSlow95 == 3.0f32)) as i32));
        let mut fSlow96: f32 = f32::round(((self.fHslider92) as f32));
        let mut fSlow97: f32 = (fSlow7 - (fSlow94 + 16.0f32));
        let mut iSlow90: i32 = (((((109.66666412353516f32 * fSlow96) * fSlow97)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow98: f32 = (329.0f32 * fSlow96);
        let mut iSlow91: i32 = ((f32::round((0.3333333432674408f32 * fSlow97))) as i32);
        let mut fSlow99: f32 = f32::round(((self.fEntry89) as f32));
        let mut iSlow92: i32 = (((fSlow99 < 2.0f32)) as i32);
        let mut iSlow93: i32 = ((((fSlow99 == 0.0f32)) as i32) | (((fSlow99 == 3.0f32)) as i32));
        let mut fSlow100: f32 = f32::round(((self.fHslider90) as f32));
        let mut fSlow101: f32 = (fSlow94 + fSlow15);
        let mut iSlow94: i32 = (((((109.66666412353516f32 * fSlow100) * fSlow101)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow102: f32 = (329.0f32 * fSlow100);
        let mut iSlow95: i32 = ((f32::round((0.3333333432674408f32 * fSlow101))) as i32);
        let mut fSlow103: f32 = f32::round(((self.fHslider97) as f32));
        let mut fSlow104: f32 = f32::round(((self.fHslider98) as f32));
        let mut iSlow96: i32 = ((((fSlow104 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow97: i32 = (if ((((((fSlow104 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow96).wrapping_sub(1i32) } else { (if (((((((fSlow104 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow96).wrapping_add(1i32) } else { iSlow96 }) });
        let mut fSlow105: f32 = ((iSlow97) as f32);
        let mut fSlow106: f32 = f32::min((fSlow103 + fSlow105), 99.0f32);
        let mut iSlow98: i32 = (((fSlow106 < 77.0f32)) as i32);
        let mut iSlow99: i32 = ((f32::round(fSlow106)) as i32);
        let mut fSlow107: f32 = (20.0f32 * (99.0f32 - fSlow106));
        let mut fSlow108: f32 = f32::round(((self.fHslider83) as f32));
        let mut iSlow100: i32 = (((fSlow108 >= 20.0f32)) as i32);
        let mut fSlow109: f32 = (fSlow108 + 28.0f32);
        let mut iSlow101: i32 = ((f32::round(fSlow108)) as i32);
        let mut iSlow102: i32 = (((fSlow108 == 0.0f32)) as i32);
        let mut fSlow110: f32 = f32::round(((self.fHslider94) as f32));
        let mut fSlow111: f32 = f32::min((fSlow110 + fSlow105), 99.0f32);
        let mut iSlow103: i32 = (((fSlow111 < 77.0f32)) as i32);
        let mut iSlow104: i32 = (iSlow103 & iSlow102);
        let mut iSlow105: i32 = ((f32::round(fSlow111)) as i32);
        let mut fSlow112: f32 = (20.0f32 * (99.0f32 - fSlow111));
        let mut fSlow113: f32 = f32::round(((self.fHslider85) as f32));
        let mut fSlow114: f32 = f32::round(((self.fHslider84) as f32));
        let mut fSlow115: f32 = f32::round(((self.fHslider96) as f32));
        let mut fSlow116: f32 = f32::round(((self.fHslider95) as f32));
        let mut iSlow106: i32 = i32::min((iSlow97).wrapping_add(((41i32).wrapping_mul(((fSlow103) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow107: i32 = (((self.fConst1 * (((((iSlow106 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow106).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow108: i32 = i32::min((iSlow97).wrapping_add(((41i32).wrapping_mul(((fSlow110) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow109: i32 = (((self.fConst1 * (((((iSlow108 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow108).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow110: i32 = ((f32::round(f32::round(((self.fHslider99) as f32)))) as i32);
        let mut iSlow111: i32 = ((f32::round(((self.fCheckbox100) as f32))) as i32);
        let mut fSlow117: f32 = f32::round(((self.fHslider101) as f32));
        let mut fSlow118: f32 = f32::round(((self.fHslider103) as f32));
        let mut iSlow112: i32 = ((f32::round(((self.fHslider102) as f32))) as i32);
        let mut fSlow119: f32 = ((if (fSlow117 > 0.0f32) { (13457.0f32 * fSlow117) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow118 + (((100i32).wrapping_mul((iSlow112 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow120: f32 = (((if ((((fSlow118) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow118) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow113: i32 = ((f32::round((((iSlow112 & 31i32)) as f32))) as i32);
        let mut fSlow121: f32 = (fSlow44 * (((72267.4453125f32 * fSlow117) * fSlow45) + 24204406.0f32));
        let mut fSlow122: f32 = f32::round(((self.fHslider65) as f32));
        let mut iSlow114: i32 = (((fSlow122 >= 20.0f32)) as i32);
        let mut fSlow123: f32 = (fSlow122 + 28.0f32);
        let mut iSlow115: i32 = ((f32::round(fSlow122)) as i32);
        let mut fSlow124: f32 = f32::round(((self.fHslider72) as f32));
        let mut fSlow125: f32 = f32::round(((self.fHslider66) as f32));
        let mut iSlow116: i32 = (((fSlow125 >= 20.0f32)) as i32);
        let mut fSlow126: f32 = (fSlow125 + 28.0f32);
        let mut iSlow117: i32 = ((f32::round(fSlow125)) as i32);
        let mut fSlow127: f32 = f32::round(((self.fHslider67) as f32));
        let mut iSlow118: i32 = ((((fSlow7 - (fSlow127 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow128: f32 = f32::round(((self.fEntry70) as f32));
        let mut iSlow119: i32 = (((fSlow128 < 2.0f32)) as i32);
        let mut iSlow120: i32 = ((((fSlow128 == 0.0f32)) as i32) | (((fSlow128 == 3.0f32)) as i32));
        let mut fSlow129: f32 = f32::round(((self.fHslider71) as f32));
        let mut fSlow130: f32 = (fSlow7 - (fSlow127 + 16.0f32));
        let mut iSlow121: i32 = (((((109.66666412353516f32 * fSlow129) * fSlow130)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow131: f32 = (329.0f32 * fSlow129);
        let mut iSlow122: i32 = ((f32::round((0.3333333432674408f32 * fSlow130))) as i32);
        let mut fSlow132: f32 = f32::round(((self.fEntry68) as f32));
        let mut iSlow123: i32 = (((fSlow132 < 2.0f32)) as i32);
        let mut iSlow124: i32 = ((((fSlow132 == 0.0f32)) as i32) | (((fSlow132 == 3.0f32)) as i32));
        let mut fSlow133: f32 = f32::round(((self.fHslider69) as f32));
        let mut fSlow134: f32 = (fSlow127 + fSlow15);
        let mut iSlow125: i32 = (((((109.66666412353516f32 * fSlow133) * fSlow134)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow135: f32 = (329.0f32 * fSlow133);
        let mut iSlow126: i32 = ((f32::round((0.3333333432674408f32 * fSlow134))) as i32);
        let mut fSlow136: f32 = f32::round(((self.fHslider76) as f32));
        let mut fSlow137: f32 = f32::round(((self.fHslider77) as f32));
        let mut iSlow127: i32 = ((((fSlow137 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow128: i32 = (if ((((((fSlow137 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow127).wrapping_sub(1i32) } else { (if (((((((fSlow137 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow127).wrapping_add(1i32) } else { iSlow127 }) });
        let mut fSlow138: f32 = ((iSlow128) as f32);
        let mut fSlow139: f32 = f32::min((fSlow136 + fSlow138), 99.0f32);
        let mut iSlow129: i32 = (((fSlow139 < 77.0f32)) as i32);
        let mut iSlow130: i32 = ((f32::round(fSlow139)) as i32);
        let mut fSlow140: f32 = (20.0f32 * (99.0f32 - fSlow139));
        let mut fSlow141: f32 = f32::round(((self.fHslider62) as f32));
        let mut iSlow131: i32 = (((fSlow141 >= 20.0f32)) as i32);
        let mut fSlow142: f32 = (fSlow141 + 28.0f32);
        let mut iSlow132: i32 = ((f32::round(fSlow141)) as i32);
        let mut iSlow133: i32 = (((fSlow141 == 0.0f32)) as i32);
        let mut fSlow143: f32 = f32::round(((self.fHslider73) as f32));
        let mut fSlow144: f32 = f32::min((fSlow143 + fSlow138), 99.0f32);
        let mut iSlow134: i32 = (((fSlow144 < 77.0f32)) as i32);
        let mut iSlow135: i32 = (iSlow134 & iSlow133);
        let mut iSlow136: i32 = ((f32::round(fSlow144)) as i32);
        let mut fSlow145: f32 = (20.0f32 * (99.0f32 - fSlow144));
        let mut fSlow146: f32 = f32::round(((self.fHslider64) as f32));
        let mut fSlow147: f32 = f32::round(((self.fHslider63) as f32));
        let mut fSlow148: f32 = f32::round(((self.fHslider75) as f32));
        let mut fSlow149: f32 = f32::round(((self.fHslider74) as f32));
        let mut iSlow137: i32 = i32::min((iSlow128).wrapping_add(((41i32).wrapping_mul(((fSlow136) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow138: i32 = (((self.fConst1 * (((((iSlow137 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow137).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow139: i32 = i32::min((iSlow128).wrapping_add(((41i32).wrapping_mul(((fSlow143) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow140: i32 = (((self.fConst1 * (((((iSlow139 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow139).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow141: i32 = ((f32::round(f32::round(((self.fHslider78) as f32)))) as i32);
        let mut iSlow142: i32 = ((f32::round(((self.fCheckbox79) as f32))) as i32);
        let mut fSlow150: f32 = f32::round(((self.fHslider80) as f32));
        let mut fSlow151: f32 = f32::round(((self.fHslider82) as f32));
        let mut iSlow143: i32 = ((f32::round(((self.fHslider81) as f32))) as i32);
        let mut fSlow152: f32 = ((if (fSlow150 > 0.0f32) { (13457.0f32 * fSlow150) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow151 + (((100i32).wrapping_mul((iSlow143 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow153: f32 = (((if ((((fSlow151) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow151) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow144: i32 = ((f32::round((((iSlow143 & 31i32)) as f32))) as i32);
        let mut fSlow154: f32 = (fSlow44 * (((72267.4453125f32 * fSlow150) * fSlow45) + 24204406.0f32));
        let mut fSlow155: f32 = f32::round(((self.fHslider129) as f32));
        let mut iSlow145: i32 = (((fSlow155 >= 20.0f32)) as i32);
        let mut fSlow156: f32 = (fSlow155 + 28.0f32);
        let mut iSlow146: i32 = ((f32::round(fSlow155)) as i32);
        let mut fSlow157: f32 = f32::round(((self.fHslider136) as f32));
        let mut fSlow158: f32 = f32::round(((self.fHslider130) as f32));
        let mut iSlow147: i32 = (((fSlow158 >= 20.0f32)) as i32);
        let mut fSlow159: f32 = (fSlow158 + 28.0f32);
        let mut iSlow148: i32 = ((f32::round(fSlow158)) as i32);
        let mut fSlow160: f32 = f32::round(((self.fHslider131) as f32));
        let mut iSlow149: i32 = ((((fSlow7 - (fSlow160 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow161: f32 = f32::round(((self.fEntry134) as f32));
        let mut iSlow150: i32 = (((fSlow161 < 2.0f32)) as i32);
        let mut iSlow151: i32 = ((((fSlow161 == 0.0f32)) as i32) | (((fSlow161 == 3.0f32)) as i32));
        let mut fSlow162: f32 = f32::round(((self.fHslider135) as f32));
        let mut fSlow163: f32 = (fSlow7 - (fSlow160 + 16.0f32));
        let mut iSlow152: i32 = (((((109.66666412353516f32 * fSlow162) * fSlow163)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow164: f32 = (329.0f32 * fSlow162);
        let mut iSlow153: i32 = ((f32::round((0.3333333432674408f32 * fSlow163))) as i32);
        let mut fSlow165: f32 = f32::round(((self.fEntry132) as f32));
        let mut iSlow154: i32 = (((fSlow165 < 2.0f32)) as i32);
        let mut iSlow155: i32 = ((((fSlow165 == 0.0f32)) as i32) | (((fSlow165 == 3.0f32)) as i32));
        let mut fSlow166: f32 = f32::round(((self.fHslider133) as f32));
        let mut fSlow167: f32 = (fSlow160 + fSlow15);
        let mut iSlow156: i32 = (((((109.66666412353516f32 * fSlow166) * fSlow167)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow168: f32 = (329.0f32 * fSlow166);
        let mut iSlow157: i32 = ((f32::round((0.3333333432674408f32 * fSlow167))) as i32);
        let mut fSlow169: f32 = f32::round(((self.fHslider140) as f32));
        let mut fSlow170: f32 = f32::round(((self.fHslider141) as f32));
        let mut iSlow158: i32 = ((((fSlow170 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow159: i32 = (if ((((((fSlow170 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow158).wrapping_sub(1i32) } else { (if (((((((fSlow170 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow158).wrapping_add(1i32) } else { iSlow158 }) });
        let mut fSlow171: f32 = ((iSlow159) as f32);
        let mut fSlow172: f32 = f32::min((fSlow169 + fSlow171), 99.0f32);
        let mut iSlow160: i32 = (((fSlow172 < 77.0f32)) as i32);
        let mut iSlow161: i32 = ((f32::round(fSlow172)) as i32);
        let mut fSlow173: f32 = (20.0f32 * (99.0f32 - fSlow172));
        let mut fSlow174: f32 = f32::round(((self.fHslider126) as f32));
        let mut iSlow162: i32 = (((fSlow174 >= 20.0f32)) as i32);
        let mut fSlow175: f32 = (fSlow174 + 28.0f32);
        let mut iSlow163: i32 = ((f32::round(fSlow174)) as i32);
        let mut iSlow164: i32 = (((fSlow174 == 0.0f32)) as i32);
        let mut fSlow176: f32 = f32::round(((self.fHslider137) as f32));
        let mut fSlow177: f32 = f32::min((fSlow176 + fSlow171), 99.0f32);
        let mut iSlow165: i32 = (((fSlow177 < 77.0f32)) as i32);
        let mut iSlow166: i32 = (iSlow165 & iSlow164);
        let mut iSlow167: i32 = ((f32::round(fSlow177)) as i32);
        let mut fSlow178: f32 = (20.0f32 * (99.0f32 - fSlow177));
        let mut fSlow179: f32 = f32::round(((self.fHslider128) as f32));
        let mut fSlow180: f32 = f32::round(((self.fHslider127) as f32));
        let mut fSlow181: f32 = f32::round(((self.fHslider139) as f32));
        let mut fSlow182: f32 = f32::round(((self.fHslider138) as f32));
        let mut iSlow168: i32 = i32::min((iSlow159).wrapping_add(((41i32).wrapping_mul(((fSlow169) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow169: i32 = (((self.fConst1 * (((((iSlow168 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow168).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow170: i32 = i32::min((iSlow159).wrapping_add(((41i32).wrapping_mul(((fSlow176) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow171: i32 = (((self.fConst1 * (((((iSlow170 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow170).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow172: i32 = ((f32::round(f32::round(((self.fHslider142) as f32)))) as i32);
        let mut iSlow173: i32 = ((f32::round(((self.fCheckbox143) as f32))) as i32);
        let mut fSlow183: f32 = f32::round(((self.fHslider144) as f32));
        let mut fSlow184: f32 = f32::round(((self.fHslider146) as f32));
        let mut iSlow174: i32 = ((f32::round(((self.fHslider145) as f32))) as i32);
        let mut fSlow185: f32 = ((if (fSlow183 > 0.0f32) { (13457.0f32 * fSlow183) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow184 + (((100i32).wrapping_mul((iSlow174 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow186: f32 = (((if ((((fSlow184) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow184) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow175: i32 = ((f32::round((((iSlow174 & 31i32)) as f32))) as i32);
        let mut fSlow187: f32 = (fSlow44 * (((72267.4453125f32 * fSlow183) * fSlow45) + 24204406.0f32));
        let mut fSlow188: f32 = f32::round(((self.fHslider107) as f32));
        let mut iSlow176: i32 = (((fSlow188 >= 20.0f32)) as i32);
        let mut fSlow189: f32 = (fSlow188 + 28.0f32);
        let mut iSlow177: i32 = ((f32::round(fSlow188)) as i32);
        let mut fSlow190: f32 = f32::round(((self.fHslider114) as f32));
        let mut fSlow191: f32 = f32::round(((self.fHslider108) as f32));
        let mut iSlow178: i32 = (((fSlow191 >= 20.0f32)) as i32);
        let mut fSlow192: f32 = (fSlow191 + 28.0f32);
        let mut iSlow179: i32 = ((f32::round(fSlow191)) as i32);
        let mut fSlow193: f32 = f32::round(((self.fHslider109) as f32));
        let mut iSlow180: i32 = ((((fSlow7 - (fSlow193 + 17.0f32)) >= 0.0f32)) as i32);
        let mut fSlow194: f32 = f32::round(((self.fEntry112) as f32));
        let mut iSlow181: i32 = (((fSlow194 < 2.0f32)) as i32);
        let mut iSlow182: i32 = ((((fSlow194 == 0.0f32)) as i32) | (((fSlow194 == 3.0f32)) as i32));
        let mut fSlow195: f32 = f32::round(((self.fHslider113) as f32));
        let mut fSlow196: f32 = (fSlow7 - (fSlow193 + 16.0f32));
        let mut iSlow183: i32 = (((((109.66666412353516f32 * fSlow195) * fSlow196)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow197: f32 = (329.0f32 * fSlow195);
        let mut iSlow184: i32 = ((f32::round((0.3333333432674408f32 * fSlow196))) as i32);
        let mut fSlow198: f32 = f32::round(((self.fEntry110) as f32));
        let mut iSlow185: i32 = (((fSlow198 < 2.0f32)) as i32);
        let mut iSlow186: i32 = ((((fSlow198 == 0.0f32)) as i32) | (((fSlow198 == 3.0f32)) as i32));
        let mut fSlow199: f32 = f32::round(((self.fHslider111) as f32));
        let mut fSlow200: f32 = (fSlow193 + fSlow15);
        let mut iSlow187: i32 = (((((109.66666412353516f32 * fSlow199) * fSlow200)) as i32)).wrapping_shr((12i32) as u32);
        let mut fSlow201: f32 = (329.0f32 * fSlow199);
        let mut iSlow188: i32 = ((f32::round((0.3333333432674408f32 * fSlow200))) as i32);
        let mut fSlow202: f32 = f32::round(((self.fHslider118) as f32));
        let mut fSlow203: f32 = f32::round(((self.fHslider119) as f32));
        let mut iSlow189: i32 = ((((fSlow203 * fSlow20)) as i32)).wrapping_shr((3i32) as u32);
        let mut iSlow190: i32 = (if ((((((fSlow203 == 3.0f32)) as i32) & iSlow16)) != 0) { (iSlow189).wrapping_sub(1i32) } else { (if (((((((fSlow203 == 7.0f32)) as i32) & iSlow18) & iSlow19)) != 0) { (iSlow189).wrapping_add(1i32) } else { iSlow189 }) });
        let mut fSlow204: f32 = ((iSlow190) as f32);
        let mut fSlow205: f32 = f32::min((fSlow202 + fSlow204), 99.0f32);
        let mut iSlow191: i32 = (((fSlow205 < 77.0f32)) as i32);
        let mut iSlow192: i32 = ((f32::round(fSlow205)) as i32);
        let mut fSlow206: f32 = (20.0f32 * (99.0f32 - fSlow205));
        let mut fSlow207: f32 = f32::round(((self.fHslider104) as f32));
        let mut iSlow193: i32 = (((fSlow207 >= 20.0f32)) as i32);
        let mut fSlow208: f32 = (fSlow207 + 28.0f32);
        let mut iSlow194: i32 = ((f32::round(fSlow207)) as i32);
        let mut iSlow195: i32 = (((fSlow207 == 0.0f32)) as i32);
        let mut fSlow209: f32 = f32::round(((self.fHslider115) as f32));
        let mut fSlow210: f32 = f32::min((fSlow209 + fSlow204), 99.0f32);
        let mut iSlow196: i32 = (((fSlow210 < 77.0f32)) as i32);
        let mut iSlow197: i32 = (iSlow196 & iSlow195);
        let mut iSlow198: i32 = ((f32::round(fSlow210)) as i32);
        let mut fSlow211: f32 = (20.0f32 * (99.0f32 - fSlow210));
        let mut fSlow212: f32 = f32::round(((self.fHslider106) as f32));
        let mut fSlow213: f32 = f32::round(((self.fHslider105) as f32));
        let mut fSlow214: f32 = f32::round(((self.fHslider117) as f32));
        let mut fSlow215: f32 = f32::round(((self.fHslider116) as f32));
        let mut iSlow199: i32 = i32::min((iSlow190).wrapping_add(((41i32).wrapping_mul(((fSlow202) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow200: i32 = (((self.fConst1 * (((((iSlow199 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow199).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow201: i32 = i32::min((iSlow190).wrapping_add(((41i32).wrapping_mul(((fSlow209) as i32))).wrapping_shr((6i32) as u32)), 63i32);
        let mut iSlow202: i32 = (((self.fConst1 * (((((iSlow201 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iSlow201).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32);
        let mut iSlow203: i32 = ((f32::round(f32::round(((self.fHslider120) as f32)))) as i32);
        let mut iSlow204: i32 = ((f32::round(((self.fCheckbox121) as f32))) as i32);
        let mut fSlow216: f32 = f32::round(((self.fHslider122) as f32));
        let mut fSlow217: f32 = f32::round(((self.fHslider124) as f32));
        let mut iSlow205: i32 = ((f32::round(((self.fHslider123) as f32))) as i32);
        let mut fSlow218: f32 = ((if (fSlow216 > 0.0f32) { (13457.0f32 * fSlow216) } else { 0.0f32 }) + ((((((4458616.0f32 * (fSlow217 + (((100i32).wrapping_mul((iSlow205 & 3i32))) as f32)))) as i32)).wrapping_shr((3i32) as u32)) as f32));
        let mut fSlow219: f32 = (((if ((((fSlow217) as i32)) != 0) { ((f32::floor(((24204406.0f32 * f32::ln(((0.009999999776482582f32 * fSlow217) + 1.0f32))) + 0.5f32))) as i32) } else { 0i32 })) as f32);
        let mut iSlow206: i32 = ((f32::round((((iSlow205 & 31i32)) as f32))) as i32);
        let mut fSlow220: f32 = (fSlow44 * (((72267.4453125f32 * fSlow216) * fSlow45) + 24204406.0f32));
        let mut fSlow221: f32 = f32::round(((self.fHslider125) as f32));
        let mut fSlow222: f32 = (if (fSlow221 == 0.0f32) { 0.0f32 } else { f32::powf(2.0f32, (fSlow221 - 7.0f32)) });
        let mut outputs_iter = outputs.iter_mut();
        let output0 = outputs_iter.nth(0).expect("missing output channel");
        let output1 = outputs_iter.nth(0).expect("missing output channel");
        for i0 in 0..count {
            self.fVec12[(0i32) as usize] = fSlow0;
            let mut iTemp0: i32 = ((((((fSlow0 < self.fVec12[(1i32) as usize])) as i32) >= 1i32)) as i32);
            let mut fTemp0: f32 = (((iTbl242[(i32::min(iSlow2, 63i32)) as usize]).wrapping_sub(239i32)) as f32);
            let mut iTemp1: i32 = (if ((iSlow7) != 0) { iSlow8 } else { ((((fSlow12 * ((iTbl129[(i32::max(i32::min(32i32, iSlow9), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp2: i32 = (if ((iSlow11) != 0) { iSlow12 } else { ((((fSlow17 * ((iTbl129[(i32::max(i32::min(32i32, iSlow13), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp1: f32 = f32::max((((((((((fSlow3 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow3) != 0) { fSlow5 } else { ((iTbl59[(i32::min(iSlow4, 19i32)) as usize]) as f32) }) + (((if ((iSlow5) != 0) { (if ((iSlow6) != 0) { (-1i32).wrapping_mul(iTemp1) } else { iTemp1 }) } else { (if ((iSlow10) != 0) { (-1i32).wrapping_mul(iTemp2) } else { iTemp2 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp3: i32 = (((f32::max((((((((((if ((iSlow0) != 0) { fSlow2 } else { ((iTbl59[(i32::min(iSlow1, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp1) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp4: i32 = (((fSlow0 > self.fVec12[(1i32) as usize])) as i32);
            let mut iTemp5: i32 = (((iTemp4 >= 1i32)) as i32);
            let mut iTemp6: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec15971[(1i32) as usize] });
            let mut iTemp7: i32 = (((f32::max((((((((((if ((iSlow23) != 0) { fSlow25 } else { ((iTbl59[(i32::min(iSlow24, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp1) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp2: f32 = (if ((iSlow26) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow28), 0i32)) as usize]) as f32) } else { fSlow28 });
            let mut iTemp8: i32 = (if ((iTemp0) != 0) { (if (iTemp3 == iTemp6) { (((self.fConst1 * (if ((iSlow21) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow22), 0i32)) as usize]) as f32) } else { fSlow23 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp7 == 0i32)) as i32) | iSlow25)) != 0) { (((self.fConst1 * (if ((iSlow27) != 0) { (0.05000000074505806f32 * fTemp2) } else { fTemp2 }))) as i32) } else { 0i32 }) } else { self.iRec15971_6[(1i32) as usize] }) });
            let mut iTemp9: i32 = (((iTemp8 != 0i32)) as i32);
            let mut iTemp10: i32 = ((((iTemp9 & (((iTemp8 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp11: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec15971_1[(1i32) as usize] }) });
            let mut iTemp12: i32 = (iTemp11).wrapping_add(1i32);
            let mut iTemp13: i32 = (if ((iTemp10) != 0) { iTemp12 } else { iTemp11 });
            let mut iTemp14: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec15971_5[(1i32) as usize] }) });
            let mut iTemp15: i32 = (((((((iTemp13 < 3i32)) as i32) | ((((iTemp13 < 4i32)) as i32) & (iTemp14 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp16: i32 = ((((((iTemp12 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp17: i32 = (((iTemp12 >= 2i32)) as i32);
            let mut iTemp18: i32 = (((iTemp12 >= 3i32)) as i32);
            let mut iTemp19: i32 = (((iTemp12 >= 1i32)) as i32);
            let mut fTemp3: f32 = (if ((iTemp17) != 0) { (if ((iTemp18) != 0) { fSlow1 } else { fSlow29 }) } else { (if ((iTemp19) != 0) { fSlow30 } else { fSlow24 }) });
            let mut iTemp20: i32 = (((f32::max((((((((((if (fTemp3 >= 20.0f32) { (fTemp3 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp3)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp1) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp21: i32 = (((iTemp12 == 0i32)) as i32);
            let mut iTemp22: i32 = (((fTemp3 == 0.0f32)) as i32);
            let mut fTemp4: f32 = (if ((iTemp17) != 0) { (if ((iTemp18) != 0) { fSlow18 } else { fSlow31 }) } else { (if ((iTemp19) != 0) { fSlow32 } else { fSlow26 }) });
            let mut fTemp5: f32 = f32::min((fSlow21 + fTemp4), 99.0f32);
            let mut iTemp23: i32 = (((fTemp5 < 77.0f32)) as i32);
            let mut fTemp6: f32 = (if ((iTemp23) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp5)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp5)) });
            let mut iTemp24: i32 = (if ((iTemp10) != 0) { (if ((iTemp16) != 0) { (if ((((((iTemp20 == iTemp6)) as i32) | (iTemp21 & iTemp22))) != 0) { (((self.fConst1 * (if ((((iTemp23 & iTemp21) & iTemp22)) != 0) { (0.05000000074505806f32 * fTemp6) } else { fTemp6 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp8).wrapping_sub((if ((iTemp9) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp25: i32 = (if ((iTemp0) != 0) { (((iTemp3 > iTemp6)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp7 > 0i32)) as i32) } else { self.iRec15971_3[(1i32) as usize] }) });
            let mut iTemp26: i32 = (if ((iTemp10) != 0) { (if ((iTemp16) != 0) { (((iTemp20 > iTemp6)) as i32) } else { iTemp25 }) } else { iTemp25 });
            let mut iTemp27: i32 = ((((iTemp24 == 0i32)) as i32)).wrapping_mul(((((iTemp26 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp28: i32 = (((iTemp27 >= 2i32)) as i32);
            let mut iTemp29: i32 = i32::min((iSlow20).wrapping_add(((41i32).wrapping_mul(((fTemp4) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp30: i32 = (if ((iTemp0) != 0) { iSlow30 } else { (if ((iTemp5) != 0) { iSlow32 } else { self.iRec15971_4[(1i32) as usize] }) });
            let mut iTemp31: i32 = (if ((iTemp10) != 0) { (if ((iTemp16) != 0) { (((self.fConst1 * (((((iTemp29 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp29).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp30 }) } else { iTemp30 });
            let mut iTemp32: i32 = (iTemp6).wrapping_sub(iTemp31);
            let mut iTemp33: i32 = (if ((iTemp0) != 0) { iTemp3 } else { (if ((iTemp5) != 0) { iTemp7 } else { self.iRec15971_2[(1i32) as usize] }) });
            let mut iTemp34: i32 = (if ((iTemp10) != 0) { (if ((iTemp16) != 0) { iTemp20 } else { iTemp33 }) } else { iTemp33 });
            let mut iTemp35: i32 = ((((((iTemp32 <= iTemp34)) as i32) >= 1i32)) as i32);
            let mut iTemp36: i32 = (((iTemp27 >= 1i32)) as i32);
            let mut iTemp37: i32 = i32::max(112459776i32, iTemp6);
            let mut iTemp38: i32 = (iTemp37).wrapping_add((((285212672i32).wrapping_sub(iTemp37)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp31));
            let mut iTemp39: i32 = ((((((iTemp38 >= iTemp34)) as i32) >= 1i32)) as i32);
            let mut iRecBody8: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp34 } else { iTemp32 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp34 } else { iTemp38 }) } else { iTemp6 }) }) } else { iTemp6 });
            let mut iTemp40: i32 = (iTemp13).wrapping_add(1i32);
            let mut iRecBody9: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp40 } else { iTemp13 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp40 } else { iTemp13 }) } else { iTemp13 }) }) } else { iTemp13 });
            let mut iTemp41: i32 = ((((((iTemp40 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp42: i32 = (((iTemp40 >= 2i32)) as i32);
            let mut iTemp43: i32 = (((iTemp40 >= 3i32)) as i32);
            let mut iTemp44: i32 = (((iTemp40 >= 1i32)) as i32);
            let mut fTemp7: f32 = (if ((iTemp42) != 0) { (if ((iTemp43) != 0) { fSlow1 } else { fSlow29 }) } else { (if ((iTemp44) != 0) { fSlow30 } else { fSlow24 }) });
            let mut iTemp45: i32 = (((f32::max(((fTemp1 + (((((((if (fTemp7 >= 20.0f32) { (fTemp7 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp7)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp46: i32 = (if ((iTemp41) != 0) { iTemp45 } else { iTemp34 });
            let mut iRecBody10: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp46 } else { iTemp34 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp46 } else { iTemp34 }) } else { iTemp34 }) }) } else { iTemp34 });
            let mut iTemp47: i32 = (if ((iTemp41) != 0) { (((iTemp45 > iTemp34)) as i32) } else { iTemp26 });
            let mut iRecBody11: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp47 } else { iTemp26 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp47 } else { iTemp26 }) } else { iTemp26 }) }) } else { iTemp26 });
            let mut fTemp8: f32 = (if ((iTemp42) != 0) { (if ((iTemp43) != 0) { fSlow18 } else { fSlow31 }) } else { (if ((iTemp44) != 0) { fSlow32 } else { fSlow26 }) });
            let mut iTemp48: i32 = i32::min((iSlow20).wrapping_add(((41i32).wrapping_mul(((fTemp8) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp49: i32 = (if ((iTemp41) != 0) { (((self.fConst1 * (((((iTemp48 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp48).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp31 });
            let mut iRecBody12: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp49 } else { iTemp31 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp49 } else { iTemp31 }) } else { iTemp31 }) }) } else { iTemp31 });
            let mut iRecBody13: i32 = iTemp14;
            let mut iTemp50: i32 = (((iTemp40 == 0i32)) as i32);
            let mut iTemp51: i32 = (((fTemp7 == 0.0f32)) as i32);
            let mut fTemp9: f32 = f32::min((fSlow21 + fTemp8), 99.0f32);
            let mut iTemp52: i32 = (((fTemp9 < 77.0f32)) as i32);
            let mut fTemp10: f32 = (if ((iTemp52) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp9)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp9)) });
            let mut iTemp53: i32 = (if ((iTemp41) != 0) { (if ((((((iTemp45 == iTemp34)) as i32) | (iTemp50 & iTemp51))) != 0) { (((self.fConst1 * (if ((((iTemp52 & iTemp50) & iTemp51)) != 0) { (0.05000000074505806f32 * fTemp10) } else { fTemp10 }))) as i32) } else { 0i32 }) } else { iTemp24 });
            let mut iRecBody14: i32 = (if ((iTemp15) != 0) { (if ((iTemp28) != 0) { (if ((iTemp35) != 0) { iTemp53 } else { iTemp24 }) } else { (if ((iTemp36) != 0) { (if ((iTemp39) != 0) { iTemp53 } else { iTemp24 }) } else { iTemp24 }) }) } else { iTemp24 });
            self.iRec15971[(0i32) as usize] = iRecBody8;
            self.iRec15971_1[(0i32) as usize] = iRecBody9;
            self.iRec15971_2[(0i32) as usize] = iRecBody10;
            self.iRec15971_3[(0i32) as usize] = iRecBody11;
            self.iRec15971_4[(0i32) as usize] = iRecBody12;
            self.iRec15971_5[(0i32) as usize] = iRecBody13;
            self.iRec15971_6[(0i32) as usize] = iRecBody14;
            let mut fTemp11: f32 = (if (((iSlow34 & iTemp4)) != 0) { 0.0f32 } else { self.fRec16024[(1i32) as usize] });
            let mut fTemp12: f32 = f32::floor((fSlow35 + fTemp11));
            let mut fTemp13: f32 = (fSlow35 + (fTemp11 - fTemp12));
            let mut fRecBody22: f32 = fTemp13;
            let mut fTemp14: f32 = (if ((iTemp4) != 0) { 0.0f32 } else { self.fRec16024_1[(1i32) as usize] });
            let mut fTemp15: f32 = (fTemp14 + (if (fTemp14 < 1.0f32) { fSlow37 } else { fSlow38 }));
            let mut iTemp54: i32 = ((((fTemp15 <= 2.0f32)) as i32)).wrapping_mul((2i32).wrapping_sub((((fTemp15 < 1.0f32)) as i32)));
            let mut iTemp55: i32 = (((iTemp54 >= 2i32)) as i32);
            let mut iTemp56: i32 = (((iTemp54 >= 1i32)) as i32);
            let mut fRecBody23: f32 = (if ((iTemp55) != 0) { fTemp15 } else { (if ((iTemp56) != 0) { fTemp15 } else { fTemp14 }) });
            let mut fRecBody24: f32 = fSlow37;
            let mut fRecBody25: f32 = fSlow38;
            let mut iTemp57: i32 = (if (fTemp13 < fSlow35) { (((179i32).wrapping_mul(self.iRec16024_4[(1i32) as usize])).wrapping_add(17i32) & 255i32) } else { self.iRec16024_4[(1i32) as usize] });
            let mut iRecBody26: i32 = (if ((iSlow38) != 0) { (if ((iSlow39) != 0) { iTemp57 } else { self.iRec16024_4[(1i32) as usize] }) } else { self.iRec16024_4[(1i32) as usize] });
            let mut iTemp58: i32 = (((fTemp13 < 0.5f32)) as i32);
            let mut fTemp16: f32 = (1.0f32 - fTemp11);
            let mut fRecBody27: f32 = (if ((iSlow38) != 0) { (if ((iSlow39) != 0) { (0.003921568859368563f32 * ((iTemp57) as f32)) } else { (if ((iSlow40) != 0) { (0.5f32 * (f32::sin((6.2831854820251465f32 * fTemp13)) + 1.0f32)) } else { (if ((iTemp58) != 0) { 1.0f32 } else { 0.0f32 }) }) }) } else { (if ((iSlow41) != 0) { fTemp13 } else { (if ((iSlow42) != 0) { ((fTemp12 + fTemp16) - fSlow35) } else { (if ((iTemp58) != 0) { (2.0f32 * fTemp13) } else { (2.0f32 * fTemp16) }) }) }) });
            let mut fRecBody28: f32 = (if ((iTemp55) != 0) { (fTemp15 - 1.0f32) } else { (if ((iTemp56) != 0) { 0.0f32 } else { 1.0f32 }) });
            self.fRec16024[(0i32) as usize] = fRecBody22;
            self.fRec16024_1[(0i32) as usize] = fRecBody23;
            self.fRec16024_2[(0i32) as usize] = fRecBody24;
            self.fRec16024_3[(0i32) as usize] = fRecBody25;
            self.iRec16024_4[(0i32) as usize] = iRecBody26;
            self.fRec16024_5[(0i32) as usize] = fRecBody27;
            self.fRec16024_6[(0i32) as usize] = fRecBody28;
            let mut iTemp59: i32 = (if ((iTemp5) != 0) { 0i32 } else { (if ((iTemp0) != 0) { 3i32 } else { self.iRec16090_1[(1i32) as usize] }) });
            let mut iTemp60: i32 = (if ((iTemp5) != 0) { 1i32 } else { (if ((iTemp0) != 0) { 0i32 } else { self.iRec16090_5[(1i32) as usize] }) });
            let mut iTemp61: i32 = (((((((iTemp59 < 3i32)) as i32) | ((((iTemp59 < 4i32)) as i32) & (1i32).wrapping_sub(iTemp60))) >= 1i32)) as i32);
            let mut fTemp17: f32 = ((iTbl1047[(iSlow48) as usize]) as f32);
            let mut iTemp62: i32 = (if ((iTemp5) != 0) { (((iTbl1047[(iSlow47) as usize] > iTbl1047[(iSlow48) as usize])) as i32) } else { (if ((iTemp0) != 0) { (((fTemp17 > self.fRec16090[(1i32) as usize])) as i32) } else { self.iRec16090_3[(1i32) as usize] }) });
            let mut iTemp63: i32 = (((iTemp62 >= 1i32)) as i32);
            let mut fTemp18: f32 = (if ((iTemp5) != 0) { fTemp17 } else { self.fRec16090[(1i32) as usize] });
            let mut fTemp19: f32 = (if ((iTemp5) != 0) { (self.fConst4 * ((iTbl1102[(iSlow49) as usize]) as f32)) } else { (if ((iTemp0) != 0) { (self.fConst4 * ((iTbl1102[(iSlow50) as usize]) as f32)) } else { self.fRec16090_4[(1i32) as usize] }) });
            let mut fTemp20: f32 = (fTemp18 + fTemp19);
            let mut iTemp64: i32 = (if ((iTemp5) != 0) { iTbl1047[(iSlow47) as usize] } else { (if ((iTemp0) != 0) { iTbl1047[(iSlow48) as usize] } else { self.iRec16090_2[(1i32) as usize] }) });
            let mut fTemp21: f32 = ((iTemp64) as f32);
            let mut iTemp65: i32 = ((((((fTemp20 >= fTemp21)) as i32) >= 1i32)) as i32);
            let mut fTemp22: f32 = (fTemp18 - fTemp19);
            let mut iTemp66: i32 = ((((((fTemp22 <= fTemp21)) as i32) >= 1i32)) as i32);
            let mut fRecBody35: f32 = (if ((iTemp61) != 0) { (if ((iTemp63) != 0) { (if ((iTemp65) != 0) { fTemp21 } else { fTemp20 }) } else { (if ((iTemp66) != 0) { fTemp21 } else { fTemp22 }) }) } else { fTemp18 });
            let mut iTemp67: i32 = (iTemp59).wrapping_add(1i32);
            let mut iRecBody36: i32 = (if ((iTemp61) != 0) { (if ((iTemp63) != 0) { (if ((iTemp65) != 0) { iTemp67 } else { iTemp59 }) } else { (if ((iTemp66) != 0) { iTemp67 } else { iTemp59 }) }) } else { iTemp59 });
            let mut iTemp68: i32 = ((((((iTemp67 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp69: i32 = (((iTemp67 >= 2i32)) as i32);
            let mut iTemp70: i32 = (((iTemp67 >= 3i32)) as i32);
            let mut iTemp71: i32 = (((iTemp67 >= 1i32)) as i32);
            let mut iTemp72: i32 = (if ((iTemp68) != 0) { iTbl1047[(((f32::round((if ((iTemp69) != 0) { (if ((iTemp70) != 0) { fSlow48 } else { fSlow51 }) } else { (if ((iTemp71) != 0) { fSlow52 } else { fSlow47 }) }))) as i32)) as usize] } else { iTemp64 });
            let mut iRecBody37: i32 = (if ((iTemp61) != 0) { (if ((iTemp63) != 0) { (if ((iTemp65) != 0) { iTemp72 } else { iTemp64 }) } else { (if ((iTemp66) != 0) { iTemp72 } else { iTemp64 }) }) } else { iTemp64 });
            let mut iTemp73: i32 = (if ((iTemp68) != 0) { (((iTbl1047[(((f32::round((if ((iTemp69) != 0) { (if ((iTemp70) != 0) { fSlow48 } else { fSlow51 }) } else { (if ((iTemp71) != 0) { fSlow52 } else { fSlow47 }) }))) as i32)) as usize] > iTemp64)) as i32) } else { iTemp62 });
            let mut iRecBody38: i32 = (if ((iTemp61) != 0) { (if ((iTemp63) != 0) { (if ((iTemp65) != 0) { iTemp73 } else { iTemp62 }) } else { (if ((iTemp66) != 0) { iTemp73 } else { iTemp62 }) }) } else { iTemp62 });
            let mut fTemp23: f32 = (if ((iTemp68) != 0) { (self.fConst4 * ((iTbl1102[(((f32::round((if ((iTemp69) != 0) { (if ((iTemp70) != 0) { fSlow50 } else { fSlow53 }) } else { (if ((iTemp71) != 0) { fSlow54 } else { fSlow49 }) }))) as i32)) as usize]) as f32)) } else { fTemp19 });
            let mut fRecBody39: f32 = (if ((iTemp61) != 0) { (if ((iTemp63) != 0) { (if ((iTemp65) != 0) { fTemp23 } else { fTemp19 }) } else { (if ((iTemp66) != 0) { fTemp23 } else { fTemp19 }) }) } else { fTemp19 });
            let mut iRecBody40: i32 = iTemp60;
            self.fRec16090[(0i32) as usize] = fRecBody35;
            self.iRec16090_1[(0i32) as usize] = iRecBody36;
            self.iRec16090_2[(0i32) as usize] = iRecBody37;
            self.iRec16090_3[(0i32) as usize] = iRecBody38;
            self.fRec16090_4[(0i32) as usize] = fRecBody39;
            self.iRec16090_5[(0i32) as usize] = iRecBody40;
            let mut iTemp74: i32 = (iSlow43 & iTemp4);
            let mut fTemp24: f32 = ((iTbl1202[(iSlow51) as usize]) as f32);
            let mut fTemp25: f32 = (self.fRec16024_5[(0i32) as usize] - 0.5f32);
            let mut fTemp26: f32 = ((524288.0f32 * self.fRec16090[(0i32) as usize]) + (16777216.0f32 * (f32::abs((fSlow55 * ((fTemp24 * self.fRec16024_6[(0i32) as usize]) * fTemp25))) * (if ((0.00390625f32 * (fTemp24 * fTemp25)) < 0.0f32) { -1.0f32 } else { 1.0f32 }))));
            let mut fTemp27: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17426 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow44) != 0) { fSlow42 } else { (fSlow43 + (((iTbl938[(iSlow46) as usize]) as f32) + fSlow46)) }) + (if ((iSlow44) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17426: f32 = (fTemp27 - f32::floor(fTemp27));
            let mut iTemp75: i32 = (if ((iSlow58) != 0) { iSlow59 } else { ((((fSlow65 * ((iTbl129[(i32::max(i32::min(32i32, iSlow60), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp76: i32 = (if ((iSlow62) != 0) { iSlow63 } else { ((((fSlow69 * ((iTbl129[(i32::max(i32::min(32i32, iSlow64), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp28: f32 = f32::max((((((((((fSlow58 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow54) != 0) { fSlow60 } else { ((iTbl59[(i32::min(iSlow55, 19i32)) as usize]) as f32) }) + (((if ((iSlow56) != 0) { (if ((iSlow57) != 0) { (-1i32).wrapping_mul(iTemp75) } else { iTemp75 }) } else { (if ((iSlow61) != 0) { (-1i32).wrapping_mul(iTemp76) } else { iTemp76 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp77: i32 = (((f32::max((((((((((if ((iSlow52) != 0) { fSlow57 } else { ((iTbl59[(i32::min(iSlow53, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp28) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp78: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec16339[(1i32) as usize] });
            let mut iTemp79: i32 = (((f32::max((((((((((if ((iSlow69) != 0) { fSlow76 } else { ((iTbl59[(i32::min(iSlow70, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp28) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp29: f32 = (if ((iSlow72) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow74), 0i32)) as usize]) as f32) } else { fSlow79 });
            let mut iTemp80: i32 = (if ((iTemp0) != 0) { (if (iTemp77 == iTemp78) { (((self.fConst1 * (if ((iSlow67) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow68), 0i32)) as usize]) as f32) } else { fSlow74 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp79 == 0i32)) as i32) | iSlow71)) != 0) { (((self.fConst1 * (if ((iSlow73) != 0) { (0.05000000074505806f32 * fTemp29) } else { fTemp29 }))) as i32) } else { 0i32 }) } else { self.iRec16339_6[(1i32) as usize] }) });
            let mut iTemp81: i32 = (((iTemp80 != 0i32)) as i32);
            let mut iTemp82: i32 = ((((iTemp81 & (((iTemp80 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp83: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec16339_1[(1i32) as usize] }) });
            let mut iTemp84: i32 = (iTemp83).wrapping_add(1i32);
            let mut iTemp85: i32 = (if ((iTemp82) != 0) { iTemp84 } else { iTemp83 });
            let mut iTemp86: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec16339_5[(1i32) as usize] }) });
            let mut iTemp87: i32 = (((((((iTemp85 < 3i32)) as i32) | ((((iTemp85 < 4i32)) as i32) & (iTemp86 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp88: i32 = ((((((iTemp84 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp89: i32 = (((iTemp84 >= 2i32)) as i32);
            let mut iTemp90: i32 = (((iTemp84 >= 3i32)) as i32);
            let mut iTemp91: i32 = (((iTemp84 >= 1i32)) as i32);
            let mut fTemp30: f32 = (if ((iTemp89) != 0) { (if ((iTemp90) != 0) { fSlow56 } else { fSlow80 }) } else { (if ((iTemp91) != 0) { fSlow81 } else { fSlow75 }) });
            let mut iTemp92: i32 = (((f32::max((((((((((if (fTemp30 >= 20.0f32) { (fTemp30 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp30)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp28) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp93: i32 = (((iTemp84 == 0i32)) as i32);
            let mut iTemp94: i32 = (((fTemp30 == 0.0f32)) as i32);
            let mut fTemp31: f32 = (if ((iTemp89) != 0) { (if ((iTemp90) != 0) { fSlow70 } else { fSlow82 }) } else { (if ((iTemp91) != 0) { fSlow83 } else { fSlow77 }) });
            let mut fTemp32: f32 = f32::min((fSlow72 + fTemp31), 99.0f32);
            let mut iTemp95: i32 = (((fTemp32 < 77.0f32)) as i32);
            let mut fTemp33: f32 = (if ((iTemp95) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp32)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp32)) });
            let mut iTemp96: i32 = (if ((iTemp82) != 0) { (if ((iTemp88) != 0) { (if ((((((iTemp92 == iTemp78)) as i32) | (iTemp93 & iTemp94))) != 0) { (((self.fConst1 * (if ((((iTemp95 & iTemp93) & iTemp94)) != 0) { (0.05000000074505806f32 * fTemp33) } else { fTemp33 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp80).wrapping_sub((if ((iTemp81) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp97: i32 = (if ((iTemp0) != 0) { (((iTemp77 > iTemp78)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp79 > 0i32)) as i32) } else { self.iRec16339_3[(1i32) as usize] }) });
            let mut iTemp98: i32 = (if ((iTemp82) != 0) { (if ((iTemp88) != 0) { (((iTemp92 > iTemp78)) as i32) } else { iTemp97 }) } else { iTemp97 });
            let mut iTemp99: i32 = ((((iTemp96 == 0i32)) as i32)).wrapping_mul(((((iTemp98 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp100: i32 = (((iTemp99 >= 2i32)) as i32);
            let mut iTemp101: i32 = i32::min((iSlow66).wrapping_add(((41i32).wrapping_mul(((fTemp31) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp102: i32 = (if ((iTemp0) != 0) { iSlow76 } else { (if ((iTemp5) != 0) { iSlow78 } else { self.iRec16339_4[(1i32) as usize] }) });
            let mut iTemp103: i32 = (if ((iTemp82) != 0) { (if ((iTemp88) != 0) { (((self.fConst1 * (((((iTemp101 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp101).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp102 }) } else { iTemp102 });
            let mut iTemp104: i32 = (iTemp78).wrapping_sub(iTemp103);
            let mut iTemp105: i32 = (if ((iTemp0) != 0) { iTemp77 } else { (if ((iTemp5) != 0) { iTemp79 } else { self.iRec16339_2[(1i32) as usize] }) });
            let mut iTemp106: i32 = (if ((iTemp82) != 0) { (if ((iTemp88) != 0) { iTemp92 } else { iTemp105 }) } else { iTemp105 });
            let mut iTemp107: i32 = ((((((iTemp104 <= iTemp106)) as i32) >= 1i32)) as i32);
            let mut iTemp108: i32 = (((iTemp99 >= 1i32)) as i32);
            let mut iTemp109: i32 = i32::max(112459776i32, iTemp78);
            let mut iTemp110: i32 = (iTemp109).wrapping_add((((285212672i32).wrapping_sub(iTemp109)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp103));
            let mut iTemp111: i32 = ((((((iTemp110 >= iTemp106)) as i32) >= 1i32)) as i32);
            let mut iRecBody48: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp106 } else { iTemp104 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp106 } else { iTemp110 }) } else { iTemp78 }) }) } else { iTemp78 });
            let mut iTemp112: i32 = (iTemp85).wrapping_add(1i32);
            let mut iRecBody49: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp112 } else { iTemp85 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp112 } else { iTemp85 }) } else { iTemp85 }) }) } else { iTemp85 });
            let mut iTemp113: i32 = ((((((iTemp112 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp114: i32 = (((iTemp112 >= 2i32)) as i32);
            let mut iTemp115: i32 = (((iTemp112 >= 3i32)) as i32);
            let mut iTemp116: i32 = (((iTemp112 >= 1i32)) as i32);
            let mut fTemp34: f32 = (if ((iTemp114) != 0) { (if ((iTemp115) != 0) { fSlow56 } else { fSlow80 }) } else { (if ((iTemp116) != 0) { fSlow81 } else { fSlow75 }) });
            let mut iTemp117: i32 = (((f32::max(((fTemp28 + (((((((if (fTemp34 >= 20.0f32) { (fTemp34 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp34)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp118: i32 = (if ((iTemp113) != 0) { iTemp117 } else { iTemp106 });
            let mut iRecBody50: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp118 } else { iTemp106 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp118 } else { iTemp106 }) } else { iTemp106 }) }) } else { iTemp106 });
            let mut iTemp119: i32 = (if ((iTemp113) != 0) { (((iTemp117 > iTemp106)) as i32) } else { iTemp98 });
            let mut iRecBody51: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp119 } else { iTemp98 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp119 } else { iTemp98 }) } else { iTemp98 }) }) } else { iTemp98 });
            let mut fTemp35: f32 = (if ((iTemp114) != 0) { (if ((iTemp115) != 0) { fSlow70 } else { fSlow82 }) } else { (if ((iTemp116) != 0) { fSlow83 } else { fSlow77 }) });
            let mut iTemp120: i32 = i32::min((iSlow66).wrapping_add(((41i32).wrapping_mul(((fTemp35) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp121: i32 = (if ((iTemp113) != 0) { (((self.fConst1 * (((((iTemp120 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp120).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp103 });
            let mut iRecBody52: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp121 } else { iTemp103 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp121 } else { iTemp103 }) } else { iTemp103 }) }) } else { iTemp103 });
            let mut iRecBody53: i32 = iTemp86;
            let mut iTemp122: i32 = (((iTemp112 == 0i32)) as i32);
            let mut iTemp123: i32 = (((fTemp34 == 0.0f32)) as i32);
            let mut fTemp36: f32 = f32::min((fSlow72 + fTemp35), 99.0f32);
            let mut iTemp124: i32 = (((fTemp36 < 77.0f32)) as i32);
            let mut fTemp37: f32 = (if ((iTemp124) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp36)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp36)) });
            let mut iTemp125: i32 = (if ((iTemp113) != 0) { (if ((((((iTemp117 == iTemp106)) as i32) | (iTemp122 & iTemp123))) != 0) { (((self.fConst1 * (if ((((iTemp124 & iTemp122) & iTemp123)) != 0) { (0.05000000074505806f32 * fTemp37) } else { fTemp37 }))) as i32) } else { 0i32 }) } else { iTemp96 });
            let mut iRecBody54: i32 = (if ((iTemp87) != 0) { (if ((iTemp100) != 0) { (if ((iTemp107) != 0) { iTemp125 } else { iTemp96 }) } else { (if ((iTemp108) != 0) { (if ((iTemp111) != 0) { iTemp125 } else { iTemp96 }) } else { iTemp96 }) }) } else { iTemp96 });
            self.iRec16339[(0i32) as usize] = iRecBody48;
            self.iRec16339_1[(0i32) as usize] = iRecBody49;
            self.iRec16339_2[(0i32) as usize] = iRecBody50;
            self.iRec16339_3[(0i32) as usize] = iRecBody51;
            self.iRec16339_4[(0i32) as usize] = iRecBody52;
            self.iRec16339_5[(0i32) as usize] = iRecBody53;
            self.iRec16339_6[(0i32) as usize] = iRecBody54;
            let mut fTemp38: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17443 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow80) != 0) { fSlow86 } else { (fSlow87 + (((iTbl938[(iSlow82) as usize]) as f32) + fSlow88)) }) + (if ((iSlow80) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17443: f32 = (fTemp38 - f32::floor(fTemp38));
            let mut iTemp126: i32 = (if ((iSlow89) != 0) { iSlow90 } else { ((((fSlow98 * ((iTbl129[(i32::max(i32::min(32i32, iSlow91), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp127: i32 = (if ((iSlow93) != 0) { iSlow94 } else { ((((fSlow102 * ((iTbl129[(i32::max(i32::min(32i32, iSlow95), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp39: f32 = f32::max((((((((((fSlow91 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow85) != 0) { fSlow93 } else { ((iTbl59[(i32::min(iSlow86, 19i32)) as usize]) as f32) }) + (((if ((iSlow87) != 0) { (if ((iSlow88) != 0) { (-1i32).wrapping_mul(iTemp126) } else { iTemp126 }) } else { (if ((iSlow92) != 0) { (-1i32).wrapping_mul(iTemp127) } else { iTemp127 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp128: i32 = (((f32::max((((((((((if ((iSlow83) != 0) { fSlow90 } else { ((iTbl59[(i32::min(iSlow84, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp39) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp129: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec16599[(1i32) as usize] });
            let mut iTemp130: i32 = (((f32::max((((((((((if ((iSlow100) != 0) { fSlow109 } else { ((iTbl59[(i32::min(iSlow101, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp39) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp40: f32 = (if ((iSlow103) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow105), 0i32)) as usize]) as f32) } else { fSlow112 });
            let mut iTemp131: i32 = (if ((iTemp0) != 0) { (if (iTemp128 == iTemp129) { (((self.fConst1 * (if ((iSlow98) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow99), 0i32)) as usize]) as f32) } else { fSlow107 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp130 == 0i32)) as i32) | iSlow102)) != 0) { (((self.fConst1 * (if ((iSlow104) != 0) { (0.05000000074505806f32 * fTemp40) } else { fTemp40 }))) as i32) } else { 0i32 }) } else { self.iRec16599_6[(1i32) as usize] }) });
            let mut iTemp132: i32 = (((iTemp131 != 0i32)) as i32);
            let mut iTemp133: i32 = ((((iTemp132 & (((iTemp131 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp134: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec16599_1[(1i32) as usize] }) });
            let mut iTemp135: i32 = (iTemp134).wrapping_add(1i32);
            let mut iTemp136: i32 = (if ((iTemp133) != 0) { iTemp135 } else { iTemp134 });
            let mut iTemp137: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec16599_5[(1i32) as usize] }) });
            let mut iTemp138: i32 = (((((((iTemp136 < 3i32)) as i32) | ((((iTemp136 < 4i32)) as i32) & (iTemp137 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp139: i32 = ((((((iTemp135 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp140: i32 = (((iTemp135 >= 2i32)) as i32);
            let mut iTemp141: i32 = (((iTemp135 >= 3i32)) as i32);
            let mut iTemp142: i32 = (((iTemp135 >= 1i32)) as i32);
            let mut fTemp41: f32 = (if ((iTemp140) != 0) { (if ((iTemp141) != 0) { fSlow89 } else { fSlow113 }) } else { (if ((iTemp142) != 0) { fSlow114 } else { fSlow108 }) });
            let mut iTemp143: i32 = (((f32::max((((((((((if (fTemp41 >= 20.0f32) { (fTemp41 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp41)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp39) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp144: i32 = (((iTemp135 == 0i32)) as i32);
            let mut iTemp145: i32 = (((fTemp41 == 0.0f32)) as i32);
            let mut fTemp42: f32 = (if ((iTemp140) != 0) { (if ((iTemp141) != 0) { fSlow103 } else { fSlow115 }) } else { (if ((iTemp142) != 0) { fSlow116 } else { fSlow110 }) });
            let mut fTemp43: f32 = f32::min((fSlow105 + fTemp42), 99.0f32);
            let mut iTemp146: i32 = (((fTemp43 < 77.0f32)) as i32);
            let mut fTemp44: f32 = (if ((iTemp146) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp43)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp43)) });
            let mut iTemp147: i32 = (if ((iTemp133) != 0) { (if ((iTemp139) != 0) { (if ((((((iTemp143 == iTemp129)) as i32) | (iTemp144 & iTemp145))) != 0) { (((self.fConst1 * (if ((((iTemp146 & iTemp144) & iTemp145)) != 0) { (0.05000000074505806f32 * fTemp44) } else { fTemp44 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp131).wrapping_sub((if ((iTemp132) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp148: i32 = (if ((iTemp0) != 0) { (((iTemp128 > iTemp129)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp130 > 0i32)) as i32) } else { self.iRec16599_3[(1i32) as usize] }) });
            let mut iTemp149: i32 = (if ((iTemp133) != 0) { (if ((iTemp139) != 0) { (((iTemp143 > iTemp129)) as i32) } else { iTemp148 }) } else { iTemp148 });
            let mut iTemp150: i32 = ((((iTemp147 == 0i32)) as i32)).wrapping_mul(((((iTemp149 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp151: i32 = (((iTemp150 >= 2i32)) as i32);
            let mut iTemp152: i32 = i32::min((iSlow97).wrapping_add(((41i32).wrapping_mul(((fTemp42) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp153: i32 = (if ((iTemp0) != 0) { iSlow107 } else { (if ((iTemp5) != 0) { iSlow109 } else { self.iRec16599_4[(1i32) as usize] }) });
            let mut iTemp154: i32 = (if ((iTemp133) != 0) { (if ((iTemp139) != 0) { (((self.fConst1 * (((((iTemp152 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp152).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp153 }) } else { iTemp153 });
            let mut iTemp155: i32 = (iTemp129).wrapping_sub(iTemp154);
            let mut iTemp156: i32 = (if ((iTemp0) != 0) { iTemp128 } else { (if ((iTemp5) != 0) { iTemp130 } else { self.iRec16599_2[(1i32) as usize] }) });
            let mut iTemp157: i32 = (if ((iTemp133) != 0) { (if ((iTemp139) != 0) { iTemp143 } else { iTemp156 }) } else { iTemp156 });
            let mut iTemp158: i32 = ((((((iTemp155 <= iTemp157)) as i32) >= 1i32)) as i32);
            let mut iTemp159: i32 = (((iTemp150 >= 1i32)) as i32);
            let mut iTemp160: i32 = i32::max(112459776i32, iTemp129);
            let mut iTemp161: i32 = (iTemp160).wrapping_add((((285212672i32).wrapping_sub(iTemp160)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp154));
            let mut iTemp162: i32 = ((((((iTemp161 >= iTemp157)) as i32) >= 1i32)) as i32);
            let mut iRecBody62: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp157 } else { iTemp155 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp157 } else { iTemp161 }) } else { iTemp129 }) }) } else { iTemp129 });
            let mut iTemp163: i32 = (iTemp136).wrapping_add(1i32);
            let mut iRecBody63: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp163 } else { iTemp136 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp163 } else { iTemp136 }) } else { iTemp136 }) }) } else { iTemp136 });
            let mut iTemp164: i32 = ((((((iTemp163 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp165: i32 = (((iTemp163 >= 2i32)) as i32);
            let mut iTemp166: i32 = (((iTemp163 >= 3i32)) as i32);
            let mut iTemp167: i32 = (((iTemp163 >= 1i32)) as i32);
            let mut fTemp45: f32 = (if ((iTemp165) != 0) { (if ((iTemp166) != 0) { fSlow89 } else { fSlow113 }) } else { (if ((iTemp167) != 0) { fSlow114 } else { fSlow108 }) });
            let mut iTemp168: i32 = (((f32::max(((fTemp39 + (((((((if (fTemp45 >= 20.0f32) { (fTemp45 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp45)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp169: i32 = (if ((iTemp164) != 0) { iTemp168 } else { iTemp157 });
            let mut iRecBody64: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp169 } else { iTemp157 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp169 } else { iTemp157 }) } else { iTemp157 }) }) } else { iTemp157 });
            let mut iTemp170: i32 = (if ((iTemp164) != 0) { (((iTemp168 > iTemp157)) as i32) } else { iTemp149 });
            let mut iRecBody65: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp170 } else { iTemp149 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp170 } else { iTemp149 }) } else { iTemp149 }) }) } else { iTemp149 });
            let mut fTemp46: f32 = (if ((iTemp165) != 0) { (if ((iTemp166) != 0) { fSlow103 } else { fSlow115 }) } else { (if ((iTemp167) != 0) { fSlow116 } else { fSlow110 }) });
            let mut iTemp171: i32 = i32::min((iSlow97).wrapping_add(((41i32).wrapping_mul(((fTemp46) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp172: i32 = (if ((iTemp164) != 0) { (((self.fConst1 * (((((iTemp171 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp171).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp154 });
            let mut iRecBody66: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp172 } else { iTemp154 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp172 } else { iTemp154 }) } else { iTemp154 }) }) } else { iTemp154 });
            let mut iRecBody67: i32 = iTemp137;
            let mut iTemp173: i32 = (((iTemp163 == 0i32)) as i32);
            let mut iTemp174: i32 = (((fTemp45 == 0.0f32)) as i32);
            let mut fTemp47: f32 = f32::min((fSlow105 + fTemp46), 99.0f32);
            let mut iTemp175: i32 = (((fTemp47 < 77.0f32)) as i32);
            let mut fTemp48: f32 = (if ((iTemp175) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp47)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp47)) });
            let mut iTemp176: i32 = (if ((iTemp164) != 0) { (if ((((((iTemp168 == iTemp157)) as i32) | (iTemp173 & iTemp174))) != 0) { (((self.fConst1 * (if ((((iTemp175 & iTemp173) & iTemp174)) != 0) { (0.05000000074505806f32 * fTemp48) } else { fTemp48 }))) as i32) } else { 0i32 }) } else { iTemp147 });
            let mut iRecBody68: i32 = (if ((iTemp138) != 0) { (if ((iTemp151) != 0) { (if ((iTemp158) != 0) { iTemp176 } else { iTemp147 }) } else { (if ((iTemp159) != 0) { (if ((iTemp162) != 0) { iTemp176 } else { iTemp147 }) } else { iTemp147 }) }) } else { iTemp147 });
            self.iRec16599[(0i32) as usize] = iRecBody62;
            self.iRec16599_1[(0i32) as usize] = iRecBody63;
            self.iRec16599_2[(0i32) as usize] = iRecBody64;
            self.iRec16599_3[(0i32) as usize] = iRecBody65;
            self.iRec16599_4[(0i32) as usize] = iRecBody66;
            self.iRec16599_5[(0i32) as usize] = iRecBody67;
            self.iRec16599_6[(0i32) as usize] = iRecBody68;
            let mut fTemp49: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17468 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow111) != 0) { fSlow119 } else { (fSlow120 + (((iTbl938[(iSlow113) as usize]) as f32) + fSlow121)) }) + (if ((iSlow111) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17468: f32 = (fTemp49 - f32::floor(fTemp49));
            let mut iTemp177: i32 = (if ((iSlow120) != 0) { iSlow121 } else { ((((fSlow131 * ((iTbl129[(i32::max(i32::min(32i32, iSlow122), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp178: i32 = (if ((iSlow124) != 0) { iSlow125 } else { ((((fSlow135 * ((iTbl129[(i32::max(i32::min(32i32, iSlow126), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp50: f32 = f32::max((((((((((fSlow124 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow116) != 0) { fSlow126 } else { ((iTbl59[(i32::min(iSlow117, 19i32)) as usize]) as f32) }) + (((if ((iSlow118) != 0) { (if ((iSlow119) != 0) { (-1i32).wrapping_mul(iTemp177) } else { iTemp177 }) } else { (if ((iSlow123) != 0) { (-1i32).wrapping_mul(iTemp178) } else { iTemp178 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp179: i32 = (((f32::max((((((((((if ((iSlow114) != 0) { fSlow123 } else { ((iTbl59[(i32::min(iSlow115, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp50) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp180: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec16851[(1i32) as usize] });
            let mut iTemp181: i32 = (((f32::max((((((((((if ((iSlow131) != 0) { fSlow142 } else { ((iTbl59[(i32::min(iSlow132, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp50) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp51: f32 = (if ((iSlow134) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow136), 0i32)) as usize]) as f32) } else { fSlow145 });
            let mut iTemp182: i32 = (if ((iTemp0) != 0) { (if (iTemp179 == iTemp180) { (((self.fConst1 * (if ((iSlow129) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow130), 0i32)) as usize]) as f32) } else { fSlow140 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp181 == 0i32)) as i32) | iSlow133)) != 0) { (((self.fConst1 * (if ((iSlow135) != 0) { (0.05000000074505806f32 * fTemp51) } else { fTemp51 }))) as i32) } else { 0i32 }) } else { self.iRec16851_6[(1i32) as usize] }) });
            let mut iTemp183: i32 = (((iTemp182 != 0i32)) as i32);
            let mut iTemp184: i32 = ((((iTemp183 & (((iTemp182 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp185: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec16851_1[(1i32) as usize] }) });
            let mut iTemp186: i32 = (iTemp185).wrapping_add(1i32);
            let mut iTemp187: i32 = (if ((iTemp184) != 0) { iTemp186 } else { iTemp185 });
            let mut iTemp188: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec16851_5[(1i32) as usize] }) });
            let mut iTemp189: i32 = (((((((iTemp187 < 3i32)) as i32) | ((((iTemp187 < 4i32)) as i32) & (iTemp188 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp190: i32 = ((((((iTemp186 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp191: i32 = (((iTemp186 >= 2i32)) as i32);
            let mut iTemp192: i32 = (((iTemp186 >= 3i32)) as i32);
            let mut iTemp193: i32 = (((iTemp186 >= 1i32)) as i32);
            let mut fTemp52: f32 = (if ((iTemp191) != 0) { (if ((iTemp192) != 0) { fSlow122 } else { fSlow146 }) } else { (if ((iTemp193) != 0) { fSlow147 } else { fSlow141 }) });
            let mut iTemp194: i32 = (((f32::max((((((((((if (fTemp52 >= 20.0f32) { (fTemp52 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp52)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp50) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp195: i32 = (((iTemp186 == 0i32)) as i32);
            let mut iTemp196: i32 = (((fTemp52 == 0.0f32)) as i32);
            let mut fTemp53: f32 = (if ((iTemp191) != 0) { (if ((iTemp192) != 0) { fSlow136 } else { fSlow148 }) } else { (if ((iTemp193) != 0) { fSlow149 } else { fSlow143 }) });
            let mut fTemp54: f32 = f32::min((fSlow138 + fTemp53), 99.0f32);
            let mut iTemp197: i32 = (((fTemp54 < 77.0f32)) as i32);
            let mut fTemp55: f32 = (if ((iTemp197) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp54)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp54)) });
            let mut iTemp198: i32 = (if ((iTemp184) != 0) { (if ((iTemp190) != 0) { (if ((((((iTemp194 == iTemp180)) as i32) | (iTemp195 & iTemp196))) != 0) { (((self.fConst1 * (if ((((iTemp197 & iTemp195) & iTemp196)) != 0) { (0.05000000074505806f32 * fTemp55) } else { fTemp55 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp182).wrapping_sub((if ((iTemp183) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp199: i32 = (if ((iTemp0) != 0) { (((iTemp179 > iTemp180)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp181 > 0i32)) as i32) } else { self.iRec16851_3[(1i32) as usize] }) });
            let mut iTemp200: i32 = (if ((iTemp184) != 0) { (if ((iTemp190) != 0) { (((iTemp194 > iTemp180)) as i32) } else { iTemp199 }) } else { iTemp199 });
            let mut iTemp201: i32 = ((((iTemp198 == 0i32)) as i32)).wrapping_mul(((((iTemp200 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp202: i32 = (((iTemp201 >= 2i32)) as i32);
            let mut iTemp203: i32 = i32::min((iSlow128).wrapping_add(((41i32).wrapping_mul(((fTemp53) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp204: i32 = (if ((iTemp0) != 0) { iSlow138 } else { (if ((iTemp5) != 0) { iSlow140 } else { self.iRec16851_4[(1i32) as usize] }) });
            let mut iTemp205: i32 = (if ((iTemp184) != 0) { (if ((iTemp190) != 0) { (((self.fConst1 * (((((iTemp203 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp203).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp204 }) } else { iTemp204 });
            let mut iTemp206: i32 = (iTemp180).wrapping_sub(iTemp205);
            let mut iTemp207: i32 = (if ((iTemp0) != 0) { iTemp179 } else { (if ((iTemp5) != 0) { iTemp181 } else { self.iRec16851_2[(1i32) as usize] }) });
            let mut iTemp208: i32 = (if ((iTemp184) != 0) { (if ((iTemp190) != 0) { iTemp194 } else { iTemp207 }) } else { iTemp207 });
            let mut iTemp209: i32 = ((((((iTemp206 <= iTemp208)) as i32) >= 1i32)) as i32);
            let mut iTemp210: i32 = (((iTemp201 >= 1i32)) as i32);
            let mut iTemp211: i32 = i32::max(112459776i32, iTemp180);
            let mut iTemp212: i32 = (iTemp211).wrapping_add((((285212672i32).wrapping_sub(iTemp211)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp205));
            let mut iTemp213: i32 = ((((((iTemp212 >= iTemp208)) as i32) >= 1i32)) as i32);
            let mut iRecBody76: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp208 } else { iTemp206 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp208 } else { iTemp212 }) } else { iTemp180 }) }) } else { iTemp180 });
            let mut iTemp214: i32 = (iTemp187).wrapping_add(1i32);
            let mut iRecBody77: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp214 } else { iTemp187 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp214 } else { iTemp187 }) } else { iTemp187 }) }) } else { iTemp187 });
            let mut iTemp215: i32 = ((((((iTemp214 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp216: i32 = (((iTemp214 >= 2i32)) as i32);
            let mut iTemp217: i32 = (((iTemp214 >= 3i32)) as i32);
            let mut iTemp218: i32 = (((iTemp214 >= 1i32)) as i32);
            let mut fTemp56: f32 = (if ((iTemp216) != 0) { (if ((iTemp217) != 0) { fSlow122 } else { fSlow146 }) } else { (if ((iTemp218) != 0) { fSlow147 } else { fSlow141 }) });
            let mut iTemp219: i32 = (((f32::max(((fTemp50 + (((((((if (fTemp56 >= 20.0f32) { (fTemp56 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp56)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp220: i32 = (if ((iTemp215) != 0) { iTemp219 } else { iTemp208 });
            let mut iRecBody78: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp220 } else { iTemp208 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp220 } else { iTemp208 }) } else { iTemp208 }) }) } else { iTemp208 });
            let mut iTemp221: i32 = (if ((iTemp215) != 0) { (((iTemp219 > iTemp208)) as i32) } else { iTemp200 });
            let mut iRecBody79: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp221 } else { iTemp200 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp221 } else { iTemp200 }) } else { iTemp200 }) }) } else { iTemp200 });
            let mut fTemp57: f32 = (if ((iTemp216) != 0) { (if ((iTemp217) != 0) { fSlow136 } else { fSlow148 }) } else { (if ((iTemp218) != 0) { fSlow149 } else { fSlow143 }) });
            let mut iTemp222: i32 = i32::min((iSlow128).wrapping_add(((41i32).wrapping_mul(((fTemp57) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp223: i32 = (if ((iTemp215) != 0) { (((self.fConst1 * (((((iTemp222 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp222).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp205 });
            let mut iRecBody80: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp223 } else { iTemp205 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp223 } else { iTemp205 }) } else { iTemp205 }) }) } else { iTemp205 });
            let mut iRecBody81: i32 = iTemp188;
            let mut iTemp224: i32 = (((iTemp214 == 0i32)) as i32);
            let mut iTemp225: i32 = (((fTemp56 == 0.0f32)) as i32);
            let mut fTemp58: f32 = f32::min((fSlow138 + fTemp57), 99.0f32);
            let mut iTemp226: i32 = (((fTemp58 < 77.0f32)) as i32);
            let mut fTemp59: f32 = (if ((iTemp226) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp58)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp58)) });
            let mut iTemp227: i32 = (if ((iTemp215) != 0) { (if ((((((iTemp219 == iTemp208)) as i32) | (iTemp224 & iTemp225))) != 0) { (((self.fConst1 * (if ((((iTemp226 & iTemp224) & iTemp225)) != 0) { (0.05000000074505806f32 * fTemp59) } else { fTemp59 }))) as i32) } else { 0i32 }) } else { iTemp198 });
            let mut iRecBody82: i32 = (if ((iTemp189) != 0) { (if ((iTemp202) != 0) { (if ((iTemp209) != 0) { iTemp227 } else { iTemp198 }) } else { (if ((iTemp210) != 0) { (if ((iTemp213) != 0) { iTemp227 } else { iTemp198 }) } else { iTemp198 }) }) } else { iTemp198 });
            self.iRec16851[(0i32) as usize] = iRecBody76;
            self.iRec16851_1[(0i32) as usize] = iRecBody77;
            self.iRec16851_2[(0i32) as usize] = iRecBody78;
            self.iRec16851_3[(0i32) as usize] = iRecBody79;
            self.iRec16851_4[(0i32) as usize] = iRecBody80;
            self.iRec16851_5[(0i32) as usize] = iRecBody81;
            self.iRec16851_6[(0i32) as usize] = iRecBody82;
            let mut fTemp60: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17485 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow142) != 0) { fSlow152 } else { (fSlow153 + (((iTbl938[(iSlow144) as usize]) as f32) + fSlow154)) }) + (if ((iSlow142) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17485: f32 = (fTemp60 - f32::floor(fTemp60));
            let mut iTemp228: i32 = (if ((iSlow151) != 0) { iSlow152 } else { ((((fSlow164 * ((iTbl129[(i32::max(i32::min(32i32, iSlow153), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp229: i32 = (if ((iSlow155) != 0) { iSlow156 } else { ((((fSlow168 * ((iTbl129[(i32::max(i32::min(32i32, iSlow157), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp61: f32 = f32::max((((((((((fSlow157 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow147) != 0) { fSlow159 } else { ((iTbl59[(i32::min(iSlow148, 19i32)) as usize]) as f32) }) + (((if ((iSlow149) != 0) { (if ((iSlow150) != 0) { (-1i32).wrapping_mul(iTemp228) } else { iTemp228 }) } else { (if ((iSlow154) != 0) { (-1i32).wrapping_mul(iTemp229) } else { iTemp229 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp230: i32 = (((f32::max((((((((((if ((iSlow145) != 0) { fSlow156 } else { ((iTbl59[(i32::min(iSlow146, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp61) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp231: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec17112[(1i32) as usize] });
            let mut iTemp232: i32 = (((f32::max((((((((((if ((iSlow162) != 0) { fSlow175 } else { ((iTbl59[(i32::min(iSlow163, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp61) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp62: f32 = (if ((iSlow165) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow167), 0i32)) as usize]) as f32) } else { fSlow178 });
            let mut iTemp233: i32 = (if ((iTemp0) != 0) { (if (iTemp230 == iTemp231) { (((self.fConst1 * (if ((iSlow160) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow161), 0i32)) as usize]) as f32) } else { fSlow173 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp232 == 0i32)) as i32) | iSlow164)) != 0) { (((self.fConst1 * (if ((iSlow166) != 0) { (0.05000000074505806f32 * fTemp62) } else { fTemp62 }))) as i32) } else { 0i32 }) } else { self.iRec17112_6[(1i32) as usize] }) });
            let mut iTemp234: i32 = (((iTemp233 != 0i32)) as i32);
            let mut iTemp235: i32 = ((((iTemp234 & (((iTemp233 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp236: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec17112_1[(1i32) as usize] }) });
            let mut iTemp237: i32 = (iTemp236).wrapping_add(1i32);
            let mut iTemp238: i32 = (if ((iTemp235) != 0) { iTemp237 } else { iTemp236 });
            let mut iTemp239: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec17112_5[(1i32) as usize] }) });
            let mut iTemp240: i32 = (((((((iTemp238 < 3i32)) as i32) | ((((iTemp238 < 4i32)) as i32) & (iTemp239 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp241: i32 = ((((((iTemp237 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp242: i32 = (((iTemp237 >= 2i32)) as i32);
            let mut iTemp243: i32 = (((iTemp237 >= 3i32)) as i32);
            let mut iTemp244: i32 = (((iTemp237 >= 1i32)) as i32);
            let mut fTemp63: f32 = (if ((iTemp242) != 0) { (if ((iTemp243) != 0) { fSlow155 } else { fSlow179 }) } else { (if ((iTemp244) != 0) { fSlow180 } else { fSlow174 }) });
            let mut iTemp245: i32 = (((f32::max((((((((((if (fTemp63 >= 20.0f32) { (fTemp63 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp63)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp61) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp246: i32 = (((iTemp237 == 0i32)) as i32);
            let mut iTemp247: i32 = (((fTemp63 == 0.0f32)) as i32);
            let mut fTemp64: f32 = (if ((iTemp242) != 0) { (if ((iTemp243) != 0) { fSlow169 } else { fSlow181 }) } else { (if ((iTemp244) != 0) { fSlow182 } else { fSlow176 }) });
            let mut fTemp65: f32 = f32::min((fSlow171 + fTemp64), 99.0f32);
            let mut iTemp248: i32 = (((fTemp65 < 77.0f32)) as i32);
            let mut fTemp66: f32 = (if ((iTemp248) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp65)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp65)) });
            let mut iTemp249: i32 = (if ((iTemp235) != 0) { (if ((iTemp241) != 0) { (if ((((((iTemp245 == iTemp231)) as i32) | (iTemp246 & iTemp247))) != 0) { (((self.fConst1 * (if ((((iTemp248 & iTemp246) & iTemp247)) != 0) { (0.05000000074505806f32 * fTemp66) } else { fTemp66 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp233).wrapping_sub((if ((iTemp234) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp250: i32 = (if ((iTemp0) != 0) { (((iTemp230 > iTemp231)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp232 > 0i32)) as i32) } else { self.iRec17112_3[(1i32) as usize] }) });
            let mut iTemp251: i32 = (if ((iTemp235) != 0) { (if ((iTemp241) != 0) { (((iTemp245 > iTemp231)) as i32) } else { iTemp250 }) } else { iTemp250 });
            let mut iTemp252: i32 = ((((iTemp249 == 0i32)) as i32)).wrapping_mul(((((iTemp251 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp253: i32 = (((iTemp252 >= 2i32)) as i32);
            let mut iTemp254: i32 = i32::min((iSlow159).wrapping_add(((41i32).wrapping_mul(((fTemp64) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp255: i32 = (if ((iTemp0) != 0) { iSlow169 } else { (if ((iTemp5) != 0) { iSlow171 } else { self.iRec17112_4[(1i32) as usize] }) });
            let mut iTemp256: i32 = (if ((iTemp235) != 0) { (if ((iTemp241) != 0) { (((self.fConst1 * (((((iTemp254 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp254).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp255 }) } else { iTemp255 });
            let mut iTemp257: i32 = (iTemp231).wrapping_sub(iTemp256);
            let mut iTemp258: i32 = (if ((iTemp0) != 0) { iTemp230 } else { (if ((iTemp5) != 0) { iTemp232 } else { self.iRec17112_2[(1i32) as usize] }) });
            let mut iTemp259: i32 = (if ((iTemp235) != 0) { (if ((iTemp241) != 0) { iTemp245 } else { iTemp258 }) } else { iTemp258 });
            let mut iTemp260: i32 = ((((((iTemp257 <= iTemp259)) as i32) >= 1i32)) as i32);
            let mut iTemp261: i32 = (((iTemp252 >= 1i32)) as i32);
            let mut iTemp262: i32 = i32::max(112459776i32, iTemp231);
            let mut iTemp263: i32 = (iTemp262).wrapping_add((((285212672i32).wrapping_sub(iTemp262)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp256));
            let mut iTemp264: i32 = ((((((iTemp263 >= iTemp259)) as i32) >= 1i32)) as i32);
            let mut iRecBody90: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp259 } else { iTemp257 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp259 } else { iTemp263 }) } else { iTemp231 }) }) } else { iTemp231 });
            let mut iTemp265: i32 = (iTemp238).wrapping_add(1i32);
            let mut iRecBody91: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp265 } else { iTemp238 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp265 } else { iTemp238 }) } else { iTemp238 }) }) } else { iTemp238 });
            let mut iTemp266: i32 = ((((((iTemp265 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp267: i32 = (((iTemp265 >= 2i32)) as i32);
            let mut iTemp268: i32 = (((iTemp265 >= 3i32)) as i32);
            let mut iTemp269: i32 = (((iTemp265 >= 1i32)) as i32);
            let mut fTemp67: f32 = (if ((iTemp267) != 0) { (if ((iTemp268) != 0) { fSlow155 } else { fSlow179 }) } else { (if ((iTemp269) != 0) { fSlow180 } else { fSlow174 }) });
            let mut iTemp270: i32 = (((f32::max(((fTemp61 + (((((((if (fTemp67 >= 20.0f32) { (fTemp67 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp67)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp271: i32 = (if ((iTemp266) != 0) { iTemp270 } else { iTemp259 });
            let mut iRecBody92: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp271 } else { iTemp259 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp271 } else { iTemp259 }) } else { iTemp259 }) }) } else { iTemp259 });
            let mut iTemp272: i32 = (if ((iTemp266) != 0) { (((iTemp270 > iTemp259)) as i32) } else { iTemp251 });
            let mut iRecBody93: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp272 } else { iTemp251 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp272 } else { iTemp251 }) } else { iTemp251 }) }) } else { iTemp251 });
            let mut fTemp68: f32 = (if ((iTemp267) != 0) { (if ((iTemp268) != 0) { fSlow169 } else { fSlow181 }) } else { (if ((iTemp269) != 0) { fSlow182 } else { fSlow176 }) });
            let mut iTemp273: i32 = i32::min((iSlow159).wrapping_add(((41i32).wrapping_mul(((fTemp68) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp274: i32 = (if ((iTemp266) != 0) { (((self.fConst1 * (((((iTemp273 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp273).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp256 });
            let mut iRecBody94: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp274 } else { iTemp256 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp274 } else { iTemp256 }) } else { iTemp256 }) }) } else { iTemp256 });
            let mut iRecBody95: i32 = iTemp239;
            let mut iTemp275: i32 = (((iTemp265 == 0i32)) as i32);
            let mut iTemp276: i32 = (((fTemp67 == 0.0f32)) as i32);
            let mut fTemp69: f32 = f32::min((fSlow171 + fTemp68), 99.0f32);
            let mut iTemp277: i32 = (((fTemp69 < 77.0f32)) as i32);
            let mut fTemp70: f32 = (if ((iTemp277) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp69)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp69)) });
            let mut iTemp278: i32 = (if ((iTemp266) != 0) { (if ((((((iTemp270 == iTemp259)) as i32) | (iTemp275 & iTemp276))) != 0) { (((self.fConst1 * (if ((((iTemp277 & iTemp275) & iTemp276)) != 0) { (0.05000000074505806f32 * fTemp70) } else { fTemp70 }))) as i32) } else { 0i32 }) } else { iTemp249 });
            let mut iRecBody96: i32 = (if ((iTemp240) != 0) { (if ((iTemp253) != 0) { (if ((iTemp260) != 0) { iTemp278 } else { iTemp249 }) } else { (if ((iTemp261) != 0) { (if ((iTemp264) != 0) { iTemp278 } else { iTemp249 }) } else { iTemp249 }) }) } else { iTemp249 });
            self.iRec17112[(0i32) as usize] = iRecBody90;
            self.iRec17112_1[(0i32) as usize] = iRecBody91;
            self.iRec17112_2[(0i32) as usize] = iRecBody92;
            self.iRec17112_3[(0i32) as usize] = iRecBody93;
            self.iRec17112_4[(0i32) as usize] = iRecBody94;
            self.iRec17112_5[(0i32) as usize] = iRecBody95;
            self.iRec17112_6[(0i32) as usize] = iRecBody96;
            let mut fTemp71: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17511 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow173) != 0) { fSlow185 } else { (fSlow186 + (((iTbl938[(iSlow175) as usize]) as f32) + fSlow187)) }) + (if ((iSlow173) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17511: f32 = (fTemp71 - f32::floor(fTemp71));
            let mut iTemp279: i32 = (if ((iSlow182) != 0) { iSlow183 } else { ((((fSlow197 * ((iTbl129[(i32::max(i32::min(32i32, iSlow184), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut iTemp280: i32 = (if ((iSlow186) != 0) { iSlow187 } else { ((((fSlow201 * ((iTbl129[(i32::max(i32::min(32i32, iSlow188), 0i32)) as usize]) as f32))) as i32)).wrapping_shr((15i32) as u32) });
            let mut fTemp72: f32 = f32::max((((((((((fSlow190 * fTemp0) + 7.0f32)) as i32)).wrapping_shr((3i32) as u32)).wrapping_shl((4i32) as u32)) as f32) + (32.0f32 * f32::min(((if ((iSlow178) != 0) { fSlow192 } else { ((iTbl59[(i32::min(iSlow179, 19i32)) as usize]) as f32) }) + (((if ((iSlow180) != 0) { (if ((iSlow181) != 0) { (-1i32).wrapping_mul(iTemp279) } else { iTemp279 }) } else { (if ((iSlow185) != 0) { (-1i32).wrapping_mul(iTemp280) } else { iTemp280 }) })) as f32)), 127.0f32))), 0.0f32);
            let mut iTemp281: i32 = (((f32::max((((((((((if ((iSlow176) != 0) { fSlow189 } else { ((iTbl59[(i32::min(iSlow177, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp72) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp282: i32 = (if ((iTemp5) != 0) { 0i32 } else { self.iRec17364[(1i32) as usize] });
            let mut iTemp283: i32 = (((f32::max((((((((((if ((iSlow193) != 0) { fSlow208 } else { ((iTbl59[(i32::min(iSlow194, 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp72) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut fTemp73: f32 = (if ((iSlow196) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow198), 0i32)) as usize]) as f32) } else { fSlow211 });
            let mut iTemp284: i32 = (if ((iTemp0) != 0) { (if (iTemp281 == iTemp282) { (((self.fConst1 * (if ((iSlow191) != 0) { ((iTbl382[(i32::max(i32::min(76i32, iSlow192), 0i32)) as usize]) as f32) } else { fSlow206 }))) as i32) } else { 0i32 }) } else { (if ((iTemp5) != 0) { (if ((((((iTemp283 == 0i32)) as i32) | iSlow195)) != 0) { (((self.fConst1 * (if ((iSlow197) != 0) { (0.05000000074505806f32 * fTemp73) } else { fTemp73 }))) as i32) } else { 0i32 }) } else { self.iRec17364_6[(1i32) as usize] }) });
            let mut iTemp285: i32 = (((iTemp284 != 0i32)) as i32);
            let mut iTemp286: i32 = ((((iTemp285 & (((iTemp284 <= 1i32)) as i32)) >= 1i32)) as i32);
            let mut iTemp287: i32 = (if ((iTemp0) != 0) { 3i32 } else { (if ((iTemp5) != 0) { 0i32 } else { self.iRec17364_1[(1i32) as usize] }) });
            let mut iTemp288: i32 = (iTemp287).wrapping_add(1i32);
            let mut iTemp289: i32 = (if ((iTemp286) != 0) { iTemp288 } else { iTemp287 });
            let mut iTemp290: i32 = (if ((iTemp0) != 0) { 0i32 } else { (if ((iTemp5) != 0) { 1i32 } else { self.iRec17364_5[(1i32) as usize] }) });
            let mut iTemp291: i32 = (((((((iTemp289 < 3i32)) as i32) | ((((iTemp289 < 4i32)) as i32) & (iTemp290 ^ -1i32))) >= 1i32)) as i32);
            let mut iTemp292: i32 = ((((((iTemp288 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp293: i32 = (((iTemp288 >= 2i32)) as i32);
            let mut iTemp294: i32 = (((iTemp288 >= 3i32)) as i32);
            let mut iTemp295: i32 = (((iTemp288 >= 1i32)) as i32);
            let mut fTemp74: f32 = (if ((iTemp293) != 0) { (if ((iTemp294) != 0) { fSlow188 } else { fSlow212 }) } else { (if ((iTemp295) != 0) { fSlow213 } else { fSlow207 }) });
            let mut iTemp296: i32 = (((f32::max((((((((((if (fTemp74 >= 20.0f32) { (fTemp74 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp74)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32) + fTemp72) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp297: i32 = (((iTemp288 == 0i32)) as i32);
            let mut iTemp298: i32 = (((fTemp74 == 0.0f32)) as i32);
            let mut fTemp75: f32 = (if ((iTemp293) != 0) { (if ((iTemp294) != 0) { fSlow202 } else { fSlow214 }) } else { (if ((iTemp295) != 0) { fSlow215 } else { fSlow209 }) });
            let mut fTemp76: f32 = f32::min((fSlow204 + fTemp75), 99.0f32);
            let mut iTemp299: i32 = (((fTemp76 < 77.0f32)) as i32);
            let mut fTemp77: f32 = (if ((iTemp299) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp76)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp76)) });
            let mut iTemp300: i32 = (if ((iTemp286) != 0) { (if ((iTemp292) != 0) { (if ((((((iTemp296 == iTemp282)) as i32) | (iTemp297 & iTemp298))) != 0) { (((self.fConst1 * (if ((((iTemp299 & iTemp297) & iTemp298)) != 0) { (0.05000000074505806f32 * fTemp77) } else { fTemp77 }))) as i32) } else { 0i32 }) } else { 0i32 }) } else { (iTemp284).wrapping_sub((if ((iTemp285) != 0) { 1i32 } else { 0i32 })) });
            let mut iTemp301: i32 = (if ((iTemp0) != 0) { (((iTemp281 > iTemp282)) as i32) } else { (if ((iTemp5) != 0) { (((iTemp283 > 0i32)) as i32) } else { self.iRec17364_3[(1i32) as usize] }) });
            let mut iTemp302: i32 = (if ((iTemp286) != 0) { (if ((iTemp292) != 0) { (((iTemp296 > iTemp282)) as i32) } else { iTemp301 }) } else { iTemp301 });
            let mut iTemp303: i32 = ((((iTemp300 == 0i32)) as i32)).wrapping_mul(((((iTemp302 == 0i32)) as i32)).wrapping_add(1i32));
            let mut iTemp304: i32 = (((iTemp303 >= 2i32)) as i32);
            let mut iTemp305: i32 = i32::min((iSlow190).wrapping_add(((41i32).wrapping_mul(((fTemp75) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp306: i32 = (if ((iTemp0) != 0) { iSlow200 } else { (if ((iTemp5) != 0) { iSlow202 } else { self.iRec17364_4[(1i32) as usize] }) });
            let mut iTemp307: i32 = (if ((iTemp286) != 0) { (if ((iTemp292) != 0) { (((self.fConst1 * (((((iTemp305 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp305).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp306 }) } else { iTemp306 });
            let mut iTemp308: i32 = (iTemp282).wrapping_sub(iTemp307);
            let mut iTemp309: i32 = (if ((iTemp0) != 0) { iTemp281 } else { (if ((iTemp5) != 0) { iTemp283 } else { self.iRec17364_2[(1i32) as usize] }) });
            let mut iTemp310: i32 = (if ((iTemp286) != 0) { (if ((iTemp292) != 0) { iTemp296 } else { iTemp309 }) } else { iTemp309 });
            let mut iTemp311: i32 = ((((((iTemp308 <= iTemp310)) as i32) >= 1i32)) as i32);
            let mut iTemp312: i32 = (((iTemp303 >= 1i32)) as i32);
            let mut iTemp313: i32 = i32::max(112459776i32, iTemp282);
            let mut iTemp314: i32 = (iTemp313).wrapping_add((((285212672i32).wrapping_sub(iTemp313)).wrapping_shr((24i32) as u32)).wrapping_mul(iTemp307));
            let mut iTemp315: i32 = ((((((iTemp314 >= iTemp310)) as i32) >= 1i32)) as i32);
            let mut iRecBody104: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp310 } else { iTemp308 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp310 } else { iTemp314 }) } else { iTemp282 }) }) } else { iTemp282 });
            let mut iTemp316: i32 = (iTemp289).wrapping_add(1i32);
            let mut iRecBody105: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp316 } else { iTemp289 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp316 } else { iTemp289 }) } else { iTemp289 }) }) } else { iTemp289 });
            let mut iTemp317: i32 = ((((((iTemp316 < 4i32)) as i32) >= 1i32)) as i32);
            let mut iTemp318: i32 = (((iTemp316 >= 2i32)) as i32);
            let mut iTemp319: i32 = (((iTemp316 >= 3i32)) as i32);
            let mut iTemp320: i32 = (((iTemp316 >= 1i32)) as i32);
            let mut fTemp78: f32 = (if ((iTemp318) != 0) { (if ((iTemp319) != 0) { fSlow188 } else { fSlow212 }) } else { (if ((iTemp320) != 0) { fSlow213 } else { fSlow207 }) });
            let mut iTemp321: i32 = (((f32::max(((fTemp72 + (((((((if (fTemp78 >= 20.0f32) { (fTemp78 + 28.0f32) } else { ((iTbl59[(i32::min(((f32::round(fTemp78)) as i32), 19i32)) as usize]) as f32) })) as i32)).wrapping_shr((1i32) as u32)).wrapping_shl((6i32) as u32)) as f32)) - 4256.0f32), 16.0f32)) as i32)).wrapping_shl((16i32) as u32);
            let mut iTemp322: i32 = (if ((iTemp317) != 0) { iTemp321 } else { iTemp310 });
            let mut iRecBody106: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp322 } else { iTemp310 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp322 } else { iTemp310 }) } else { iTemp310 }) }) } else { iTemp310 });
            let mut iTemp323: i32 = (if ((iTemp317) != 0) { (((iTemp321 > iTemp310)) as i32) } else { iTemp302 });
            let mut iRecBody107: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp323 } else { iTemp302 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp323 } else { iTemp302 }) } else { iTemp302 }) }) } else { iTemp302 });
            let mut fTemp79: f32 = (if ((iTemp318) != 0) { (if ((iTemp319) != 0) { fSlow202 } else { fSlow214 }) } else { (if ((iTemp320) != 0) { fSlow215 } else { fSlow209 }) });
            let mut iTemp324: i32 = i32::min((iSlow190).wrapping_add(((41i32).wrapping_mul(((fTemp79) as i32))).wrapping_shr((6i32) as u32)), 63i32);
            let mut iTemp325: i32 = (if ((iTemp317) != 0) { (((self.fConst1 * (((((iTemp324 & 3i32)).wrapping_add(4i32)).wrapping_shl((((iTemp324).wrapping_shr((2i32) as u32)).wrapping_add(2i32)) as u32)) as f32))) as i32) } else { iTemp307 });
            let mut iRecBody108: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp325 } else { iTemp307 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp325 } else { iTemp307 }) } else { iTemp307 }) }) } else { iTemp307 });
            let mut iRecBody109: i32 = iTemp290;
            let mut iTemp326: i32 = (((iTemp316 == 0i32)) as i32);
            let mut iTemp327: i32 = (((fTemp78 == 0.0f32)) as i32);
            let mut fTemp80: f32 = f32::min((fSlow204 + fTemp79), 99.0f32);
            let mut iTemp328: i32 = (((fTemp80 < 77.0f32)) as i32);
            let mut fTemp81: f32 = (if ((iTemp328) != 0) { ((iTbl382[(i32::max(i32::min(76i32, ((f32::round(fTemp80)) as i32)), 0i32)) as usize]) as f32) } else { (20.0f32 * (99.0f32 - fTemp80)) });
            let mut iTemp329: i32 = (if ((iTemp317) != 0) { (if ((((((iTemp321 == iTemp310)) as i32) | (iTemp326 & iTemp327))) != 0) { (((self.fConst1 * (if ((((iTemp328 & iTemp326) & iTemp327)) != 0) { (0.05000000074505806f32 * fTemp81) } else { fTemp81 }))) as i32) } else { 0i32 }) } else { iTemp300 });
            let mut iRecBody110: i32 = (if ((iTemp291) != 0) { (if ((iTemp304) != 0) { (if ((iTemp311) != 0) { iTemp329 } else { iTemp300 }) } else { (if ((iTemp312) != 0) { (if ((iTemp315) != 0) { iTemp329 } else { iTemp300 }) } else { iTemp300 }) }) } else { iTemp300 });
            self.iRec17364[(0i32) as usize] = iRecBody104;
            self.iRec17364_1[(0i32) as usize] = iRecBody105;
            self.iRec17364_2[(0i32) as usize] = iRecBody106;
            self.iRec17364_3[(0i32) as usize] = iRecBody107;
            self.iRec17364_4[(0i32) as usize] = iRecBody108;
            self.iRec17364_5[(0i32) as usize] = iRecBody109;
            self.iRec17364_6[(0i32) as usize] = iRecBody110;
            let mut fTemp82: f32 = (if ((iTemp74) != 0) { 0.0f32 } else { (self.fRec17528 + (self.fConst2 * f32::powf(2.0f32, (5.960464477539063e-8f32 * ((if ((iSlow204) != 0) { fSlow218 } else { (fSlow219 + (((iTbl938[(iSlow206) as usize]) as f32) + fSlow220)) }) + (if ((iSlow204) != 0) { 0.0f32 } else { fTemp26 })))))) });
            let mut fRecCur17528: f32 = (fTemp82 - f32::floor(fTemp82));
            let mut fTemp83: f32 = (1.0f32 - self.fRec16024_5[(0i32) as usize]);
            let mut fRecCur17536: f32 = (0.5f32 * (f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec17364[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow203) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec17364[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow203) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * (fRecCur17528 + (fSlow222 * self.fRec17536))))));
            let mut fTemp84: FaustFloat = (((0.5f32 * (((f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec15971[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow33) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec15971[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow33) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * (fRecCur17426 + (0.5f32 * (f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec16339[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow79) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec16339[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow79) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * fRecCur17443)))))))) + (f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec16599[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow110) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec16599[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow110) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * (fRecCur17468 + (0.5f32 * (f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec16851[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow141) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec16851[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow141) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * fRecCur17485))))))))) + (f32::powf(2.0f32, (5.960464477539063e-8f32 * (((self.iRec17112[(0i32) as usize]).wrapping_sub(((if (iTbl734[(iSlow172) as usize] != 0i32) { ((((5.960464477539063e-8f32 * (((self.iRec17112[(0i32) as usize]) as f32) * f32::exp(((fSlow33 * ((((iTbl734[(iSlow172) as usize]) as f32) * self.fRec16024_6[(0i32) as usize]) * fTemp83)) + 12.199999809265137f32)))) + 0.5f32)) as i32) } else { 0i32 })).wrapping_add(234881024i32))) as f32))) * f32::sin((6.2831854820251465f32 * (fRecCur17511 + fRecCur17536))))))) as FaustFloat);
            output0[(i0) as usize] = fTemp84;
            output1[(i0) as usize] = fTemp84;
            self.fVec12[(1i32) as usize] = self.fVec12[(0i32) as usize];
            self.iRec15971[(1i32) as usize] = self.iRec15971[(0i32) as usize];
            self.iRec15971_1[(1i32) as usize] = self.iRec15971_1[(0i32) as usize];
            self.iRec15971_2[(1i32) as usize] = self.iRec15971_2[(0i32) as usize];
            self.iRec15971_3[(1i32) as usize] = self.iRec15971_3[(0i32) as usize];
            self.iRec15971_4[(1i32) as usize] = self.iRec15971_4[(0i32) as usize];
            self.iRec15971_5[(1i32) as usize] = self.iRec15971_5[(0i32) as usize];
            self.iRec15971_6[(1i32) as usize] = self.iRec15971_6[(0i32) as usize];
            self.fRec16024[(1i32) as usize] = self.fRec16024[(0i32) as usize];
            self.fRec16024_1[(1i32) as usize] = self.fRec16024_1[(0i32) as usize];
            self.fRec16024_2[(1i32) as usize] = self.fRec16024_2[(0i32) as usize];
            self.fRec16024_3[(1i32) as usize] = self.fRec16024_3[(0i32) as usize];
            self.iRec16024_4[(1i32) as usize] = self.iRec16024_4[(0i32) as usize];
            self.fRec16024_5[(1i32) as usize] = self.fRec16024_5[(0i32) as usize];
            self.fRec16024_6[(1i32) as usize] = self.fRec16024_6[(0i32) as usize];
            self.fRec16090[(1i32) as usize] = self.fRec16090[(0i32) as usize];
            self.iRec16090_1[(1i32) as usize] = self.iRec16090_1[(0i32) as usize];
            self.iRec16090_2[(1i32) as usize] = self.iRec16090_2[(0i32) as usize];
            self.iRec16090_3[(1i32) as usize] = self.iRec16090_3[(0i32) as usize];
            self.fRec16090_4[(1i32) as usize] = self.fRec16090_4[(0i32) as usize];
            self.iRec16090_5[(1i32) as usize] = self.iRec16090_5[(0i32) as usize];
            self.fRec17426 = fRecCur17426;
            self.iRec16339[(1i32) as usize] = self.iRec16339[(0i32) as usize];
            self.iRec16339_1[(1i32) as usize] = self.iRec16339_1[(0i32) as usize];
            self.iRec16339_2[(1i32) as usize] = self.iRec16339_2[(0i32) as usize];
            self.iRec16339_3[(1i32) as usize] = self.iRec16339_3[(0i32) as usize];
            self.iRec16339_4[(1i32) as usize] = self.iRec16339_4[(0i32) as usize];
            self.iRec16339_5[(1i32) as usize] = self.iRec16339_5[(0i32) as usize];
            self.iRec16339_6[(1i32) as usize] = self.iRec16339_6[(0i32) as usize];
            self.fRec17443 = fRecCur17443;
            self.iRec16599[(1i32) as usize] = self.iRec16599[(0i32) as usize];
            self.iRec16599_1[(1i32) as usize] = self.iRec16599_1[(0i32) as usize];
            self.iRec16599_2[(1i32) as usize] = self.iRec16599_2[(0i32) as usize];
            self.iRec16599_3[(1i32) as usize] = self.iRec16599_3[(0i32) as usize];
            self.iRec16599_4[(1i32) as usize] = self.iRec16599_4[(0i32) as usize];
            self.iRec16599_5[(1i32) as usize] = self.iRec16599_5[(0i32) as usize];
            self.iRec16599_6[(1i32) as usize] = self.iRec16599_6[(0i32) as usize];
            self.fRec17468 = fRecCur17468;
            self.iRec16851[(1i32) as usize] = self.iRec16851[(0i32) as usize];
            self.iRec16851_1[(1i32) as usize] = self.iRec16851_1[(0i32) as usize];
            self.iRec16851_2[(1i32) as usize] = self.iRec16851_2[(0i32) as usize];
            self.iRec16851_3[(1i32) as usize] = self.iRec16851_3[(0i32) as usize];
            self.iRec16851_4[(1i32) as usize] = self.iRec16851_4[(0i32) as usize];
            self.iRec16851_5[(1i32) as usize] = self.iRec16851_5[(0i32) as usize];
            self.iRec16851_6[(1i32) as usize] = self.iRec16851_6[(0i32) as usize];
            self.fRec17485 = fRecCur17485;
            self.iRec17112[(1i32) as usize] = self.iRec17112[(0i32) as usize];
            self.iRec17112_1[(1i32) as usize] = self.iRec17112_1[(0i32) as usize];
            self.iRec17112_2[(1i32) as usize] = self.iRec17112_2[(0i32) as usize];
            self.iRec17112_3[(1i32) as usize] = self.iRec17112_3[(0i32) as usize];
            self.iRec17112_4[(1i32) as usize] = self.iRec17112_4[(0i32) as usize];
            self.iRec17112_5[(1i32) as usize] = self.iRec17112_5[(0i32) as usize];
            self.iRec17112_6[(1i32) as usize] = self.iRec17112_6[(0i32) as usize];
            self.fRec17511 = fRecCur17511;
            self.iRec17364[(1i32) as usize] = self.iRec17364[(0i32) as usize];
            self.iRec17364_1[(1i32) as usize] = self.iRec17364_1[(0i32) as usize];
            self.iRec17364_2[(1i32) as usize] = self.iRec17364_2[(0i32) as usize];
            self.iRec17364_3[(1i32) as usize] = self.iRec17364_3[(0i32) as usize];
            self.iRec17364_4[(1i32) as usize] = self.iRec17364_4[(0i32) as usize];
            self.iRec17364_5[(1i32) as usize] = self.iRec17364_5[(0i32) as usize];
            self.iRec17364_6[(1i32) as usize] = self.iRec17364_6[(0i32) as usize];
            self.fRec17528 = fRecCur17528;
            self.fRec17536 = fRecCur17536;
        }
    }

}
