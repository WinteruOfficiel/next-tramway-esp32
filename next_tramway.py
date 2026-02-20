import requests
import json
from collections import defaultdict
import unicodedata
import time

from appdaemon.plugins.hass import Hass

# StopTime = namedtuple(
#     "StopTime",
#     ["pattern_id", "arrival_time", "relative_arrival_time", "destination_short", "destination_long", "realtime_state"]
# )

lines_to_keep = [ "SEM:A:", "SEM:B:", "SEM:C:", "SEM:D:", "SEM:E:" ]
lines_to_show = ["A", "B", "C", "D", "E"]
lines_display_name = { "A": "Tram A", "B": "Tram B", "C": "Tram C", "D": "Tram D", "E": "Tram E" }
STOP_ID = ""

UPDATE_EVERY=20 # in seconds

class NextTramway(Hass):
    def initialize(self):
        self.run_every(self.update, "now", UPDATE_EVERY)
    
    def update(self, **kwargs):
        stops_by_line_dir = defaultdict(lambda: defaultdict(list))

        headers = {
            'Origin': 'http://localhost:'
        }

        url = f"http://data.mobilites-m.fr/api/routers/default/index/clusters/{STOP_ID}/stoptimes"
        response = requests.get(url, headers=headers)

        def sec_to_hms(sec):
            h = sec // 3600
            m = (sec % 3600) // 60
            s = sec % 60
            return f"{h:02d}:{m:02d}:{s:02d}"

        def now_sec_since_midnight():
            t = time.localtime()
            return t.tm_hour * 3600 + t.tm_min * 60 + t.tm_sec

        def relative_minutes(arrival_sec, now_sec):
            SECONDS_PER_DAY = 86400
            delta = (arrival_sec - now_sec) % SECONDS_PER_DAY
            return delta // 60           

        for stop_time in response.json():
            if any(stop_time["pattern"]["id"].startswith(line) for line in lines_to_keep):
                print(json.dumps(stop_time, indent=2))
                for stop in stop_time["times"]:
                    relative_arrival = relative_minutes(stop["realtimeArrival"], now_sec_since_midnight())
                    if relative_arrival < 0:
                        print(f"Skipping stop with arrival time in the past: {stop['realtimeArrival']} (relative: {relative_arrival} minutes)")
                        continue
                    line = (stop_time["pattern"]["id"].split(":")[1])
                    direction = stop_time["pattern"]["dir"]
                    stops_by_line_dir[line][direction].append({
                        #"pattern_id": stop_time["pattern"]["id"],
                        "line": line,
                        "dir": direction,
                        #"arrival_time": sec_to_hms(stop["realtimeArrival"]),
                        "relative_arrival_time": relative_arrival,
                        "destination_short": stop_time["pattern"]["desc"],
                        #"destination_long": stop_time["pattern"]["lastStopName"],
                        "realtime_state": stop["realtimeState"]
                    })
        
        self.set_state(
            "sensor.next_tramway",
            state= f"Update at {time.strftime("%Y-%m-%d %H:%M:%S")}",
            attributes={
                "stops": stops_by_line_dir,
                 "test": None,
                 "updateAt": time.strftime("%Y-%m-%d %H:%M:%S")
            }
        )

        self.send_mqtt(stops_by_line_dir)
    
    def send_mqtt(self, stops_by_line_by_dir):
        def gen_empty_line(stops, line):
            stops = stops_by_line_by_dir.get(line, {})

            if not stops:
                self.call_service(
                    "mqtt/publish",
                    topic=f"next-tramway/line/{line}/1",
                    payload=f"{lines_display_name.get(line, line)}\n{time.strftime("%H:%M:%S")}",
                    retain=True
                )
            
        def strip_accents(s: str) -> str:
            return ''.join(
                c for c in unicodedata.normalize('NFD', s)
                if unicodedata.category(c) != 'Mn'
            )
        def sanitize(s):
            return strip_accents(s.replace("|", " ").replace("\n", " "))

        for line in lines_to_show:
            gen_empty_line(stops_by_line_by_dir, line)

        for line, stops_by_line in stops_by_line_by_dir.items():
            for direction, stops_by_dir in stops_by_line.items():
                display_name = lines_display_name.get(line, line)
                timestamp = time.strftime("%H:%M:%S")

                passages = [
                    f"{sanitize(stop['destination_short'])[:17]}|"
                    f"{min(stop['relative_arrival_time'], 60)}|"
                    f"{'R' if stop['realtime_state'] == 'UPDATED' else 'S'}"
                    for stop in stops_by_dir
                ][:2]

                payload = "\n".join([display_name, *passages, timestamp])

                self.call_service(
                    "mqtt/publish",
                    topic=f"next-tramway/line/{line}/{direction}",
                    payload=payload,
                    retain=True
                )
