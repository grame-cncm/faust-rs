process = _ * hslider("gain", 0.5, 0, 1, 0.01)
        + vslider("v", 0, -1, 1, 0.1)
        + nentry("n", 2, 0, 10, 1)
        * button("b")
        * checkbox("c");
