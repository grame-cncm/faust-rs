BEGIN {
    FS = ","
    OFS = ","
}

NR == 1 {
    next
}

{
    status_count[$5]++
}

$5 == "ok" {
    comparable++
    cpp = $2 + 0
    rust = $3 + 0
    delta = $4 + 0
    deltas[comparable] = delta
    log_ratio_sum += log(rust / cpp)

    if (delta > 0) {
        better++
    } else if (delta < 0) {
        worse++
    } else {
        same++
    }
    if (delta <= -warn_min) {
        regressions++
    }
}

END {
    for (i = 1; i <= comparable; i++) {
        for (j = i + 1; j <= comparable; j++) {
            if (deltas[i] > deltas[j]) {
                tmp = deltas[i]
                deltas[i] = deltas[j]
                deltas[j] = tmp
            }
        }
    }

    if (comparable == 0) {
        geomean = ""
        median = ""
    } else {
        geomean = sprintf("%.2f", (exp(log_ratio_sum / comparable) - 1) * 100)
        if (comparable % 2 == 1) {
            median = sprintf("%.2f", deltas[(comparable + 1) / 2])
        } else {
            median = sprintf("%.2f", (deltas[comparable / 2] + deltas[comparable / 2 + 1]) / 2)
        }
    }

    print "comparable_dsps", "better", "worse", "same", \
        "geomean_delta_pct", "median_delta_pct", "regressions_ge_warn", \
        "unsupported_cpp", "failed_cpp", "failed_faust_rs", "failed_both", \
        "nonfinite_cpp", "nonfinite_faust_rs", "nonfinite_both"
    print comparable + 0, better + 0, worse + 0, same + 0, geomean, median, \
        regressions + 0, status_count["unsupported_cpp"] + 0, \
        status_count["failed_cpp"] + 0, status_count["failed_faust_rs"] + 0, \
        status_count["failed_both"] + 0, status_count["nonfinite_cpp"] + 0, \
        status_count["nonfinite_faust_rs"] + 0, status_count["nonfinite_both"] + 0
}
