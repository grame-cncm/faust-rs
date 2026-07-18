// P0 case: multiple outputs with mixed shapes (pure, recursive, delayed).
process = _ <: *(0.5), (+ ~ *(0.5)), @(10);
