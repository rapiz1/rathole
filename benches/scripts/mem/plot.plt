set title "Memory Usage" font ",20"

set term png small size 800,600
set key box outside

set output "mem-graph.png"

set ylabel "RSZ"
set format y '%.0s%cB'

set ytics nomirror

set yrange [0:*]

plot "frps-mem.log" using 1 with lines axes x1y1 title "frps RSZ", \
     "frpc-mem.log" using 1 with lines axes x1y1 title "frpc RSZ", \
     "ratholes-mem.log" using 1 with lines axes x1y1 title "ratholes RSZ", \
     "ratholec-mem.log" using 1 with lines axes x1y1 title "ratholec RSZ"
