dl = library("demos.lib");

//process = dl.sawtooth_demo <:
//  dl.flanger_demo : dl.phaser2_demo :> dl.spectral_level_demo <: _,_;

fx_stack =
 vgroup("[1]", dl.sawtooth_demo) <:
 vgroup("[2]", dl.flanger_demo) :
 vgroup("[3]", dl.phaser2_demo);

level_viewer(x,y) = attach(x,vgroup("[4]", dl.spectral_level_demo(x+y))),y;

process = fx_stack : level_viewer;
