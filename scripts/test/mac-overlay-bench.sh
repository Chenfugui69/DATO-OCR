#!/bin/zsh
# 量截图遮罩拖动时的开销（macOS）：帧间隔，以及这期间各进程用掉的 CPU 时间（毫秒 / 帧）。
#
#   scripts/test/mac-overlay-bench.sh <名字> <ocr|capture> <屏号> ['<选项 JSON>']
#   例：scripts/test/mac-overlay-bench.sh 基线 ocr 1
#       scripts/test/mac-overlay-bench.sh 无放大镜 ocr 1 '{"css":".cap-magnifier{display:none!important}"}'
#
# 前提：调试版已经在跑，并且日志打到 $CHENOCR_BENCH_LOG（启动时把输出重定向到这个文件）。
# 屏号就是遮罩窗口标签 capture-<N> 里的 N（启动日志里有）。选项见 overlay-bench.js。
#
# 只看帧间隔不够：60Hz 的屏上只要每帧低于 16ms 就看不出差别，而且光栅化在 GPU 进程里、
# 合成在 WindowServer 里，都不在页面的主线程上。所以同时量这几个进程的 CPU 时间。
# 遮罩会盖住屏幕几秒钟，量完自动取消。
BIN=${CHENOCR_BIN:-/Volumes/DatoCorBuild/target/debug/chenocr}
LOG=${CHENOCR_BENCH_LOG:?要先设 CHENOCR_BENCH_LOG=应用日志文件}
HERE="${0:A:h}"
NAME=$1; ACTION=$2; MONITOR=$3; OPTS=${4:-'{}'}
APP=$(pgrep -f "^$BIN" | head -1)
[ -z "$APP" ] && { echo "调试版没在运行"; exit 1; }
# 应用要是挂了，第二个实例会变成正常启动而不退出，所以每次调用都带超时
run() { perl -e 'alarm 8; exec @ARGV' "$BIN" "$@" >/dev/null 2>&1; }
cpu() { ps -o time= -p $1 2>/dev/null | awk -F'[:.]' '{ if (NF==3) print ($1*60+$2)*1000+$3*10; else if (NF==4) print (($1*60+$2)*60+$3)*1000+$4*10; else print 0 }'; }
total() { local sum=0; for p in "$@"; do sum=$((sum + $(cpu $p))); done; echo $sum; }
plain() { sed 's/\x1b\[[0-9;]*m//g' "$LOG"; }
marks() { plain | grep -c "scope=bench"; }

SCRIPT=$(mktemp -t overlay-bench).js
printf 'window.__BENCH__ = %s;\n' "$OPTS" > "$SCRIPT"; cat "$HERE/overlay-bench.js" >> "$SCRIPT"
run --action=$ACTION; sleep 1.6
# 别的应用也可能有 WebKit 进程，分不出是谁的，全算上（它们平时是闲着的）
GPU=($(pgrep -f "com.apple.WebKit.GPU")); WEB=($(pgrep -f "com.apple.WebKit.WebContent")); WS=$(pgrep -x WindowServer)
before=$(marks); run "--eval=capture-$MONITOR:$SCRIPT"
for i in $(seq 1 100); do sleep 0.1; [ "$(marks)" -gt "$before" ] && break; done
g0=$(total $GPU); w0=$(total $WEB); a0=$(cpu $APP); s0=$(cpu $WS)
for i in $(seq 1 300); do sleep 0.1; [ "$(marks)" -gt "$((before + 1))" ] && break; done
g1=$(total $GPU); w1=$(total $WEB); a1=$(cpu $APP); s1=$(cpu $WS)
run --action=cancel; rm -f "$SCRIPT"; sleep 0.7
END=$(plain | grep "scope=bench" | tail -1 | sed 's/.*前端错误：//; s/ window=.*//')
python3 - "$NAME" "$END" $((g1-g0)) $((w1-w0)) $((a1-a0)) $((s1-s0)) <<'PY'
import json, sys
name, end = sys.argv[1], json.loads(sys.argv[2])
gpu, web, app, ws = map(int, sys.argv[3:7])
if "error" in end or "n" not in end:
    sys.exit(f"{name}: 基准没跑完：{end}")
n = end["n"]
print(f"{name:<22} 帧间隔 平均 {end['avg']}ms  p95 {end['p95']}  最大 {end['max']}  超过20ms {end['slow']} 帧"
      f" | 每帧 CPU(ms)：GPU进程 {gpu/n:.2f}  网页进程 {web/n:.2f}  应用 {app/n:.2f}  WindowServer {ws/n:.2f}  合计 {(gpu+web+app+ws)/n:.2f}")
PY
