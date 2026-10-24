# Verifying the pinout

Steps to confirm the pinout in [README.md](README.md) on our own panels.

**Progress (2026-10-15):** steps 1 to 10 done on one panel, results below,
with the panel driven by hand from the header. Left out because the probes
are too thick for the HC4514 pins: their outputs, and the 74HC139 outputs
1Y2, 1Y3 and the column half to each HC4514. Also left out: which decoder
outputs feed which driver type (IC11 `083AG` or IC12 `783AG`) and the
supply current (the meter gives no reading). C5 holds no steady level, so
the column enable was tested in step 10 only: one tap of pin 19 to 5 V
flips dot 1/1 on (header pins 5, 13 and 11 high) and back (pins 5, 13, 10
and 18 high). **Next: step 11 with the adapter board**, plus the shortest
reliable pulse and the coil current, which need it too.

Steps 1 to 6 need only a multimeter in continuity mode and no power. Do them
in order; stop when a result differs from the expected one and correct the
README first.

## Chip pins used below

| Chip                 | Pins                                                              |
|----------------------|-------------------------------------------------------------------|
| 74HC4514 (24 pins)   | 1 LE, 2 A0, 3 A1, 21 A2, 22 A3, 23 /E, 12 GND, 24 VCC             |
| 74HC139 (16 pins)    | 1 /1E, 2 1A0, 3 1A1, 4–7 1Y0–1Y3, 8 GND, 15 /2E, 14 2A0, 13 2A1, 12–9 2Y0–2Y3, 16 VCC |
| 74HC02 (14 pins)     | Gates (A, B → Y): 2, 3 → 1; 5, 6 → 4; 8, 9 → 10; 11, 12 → 13; 7 GND, 14 VCC |
| TBD62783A (18 pins)  | 1–8 inputs, 18–11 outputs, 9 coil supply, 10 GND                  |
| TBD62083A (18 pins)  | 1–8 inputs, 18–11 outputs, 9 GND, 10 common                       |

The two driver types are a guess from the markings `783AG` and `083AG`.
Check them against the datasheet once the full marking is read.

## Without power

### 1. Find pin 1

Look for a square pad, a `1` or a triangle in the silkscreen at the header.
The notch of the shroud is on the side of the odd pins. Mark pin 1 on the
board.

Expected: the mark agrees with step 2. If no mark is found, step 2 decides.

### 2. Ground, 5 V and coil supply

Measure continuity from the header to the chips.

| From header pin | To                                      | Expected        |
|-----------------|-----------------------------------------|-----------------|
| 1 and 2         | HC4514 pin 12, 74HC139 pin 8            | Connected       |
| 3               | HC4514 pin 24, 74HC139 pin 16           | Connected       |
| 26              | `783AG` pin 9                           | Connected       |
| 4               | HC4514 pin 24                           | Not known       |
| 25              | `783AG` pin 9                           | Not known       |
| 3               | 26, and 1                               | Not connected   |
| 26              | 1                                       | Not connected   |

If ground turns up on pins 25 and 26 instead, pin 1 is at the other end. If
5 V is on pin 4 and the coil supply on pin 25, the two rows are numbered the
other way round, as in Christensen's schematic.

Write down what pins 4 and 25 do.

### 3. Compare the two headers

Measure continuity from each pin of one 26-pin header to the same pin of the
other.

Expected: ground, 5 V, coil supply and all address lines connected pin for
pin. Note whether pins 19 to 24 are connected straight or shifted by one;
a shift would mean each panel in a chain gets its own select line that way.

### 4. Column address

| From header pin | To                                         | Expected  |
|-----------------|--------------------------------------------|-----------|
| 13 (COL0)       | Pin 2 (A0) of four HC4514                  | Connected |
| 14 (COL1)       | Pin 3 (A1) of the same four                | Connected |
| 15 (COL2)       | Pin 21 (A2) of the same four               | Connected |
| 16 (COL3)       | An A0 input of the 74HC139 (pin 2 or 14)   | Connected |
| 17 (COL4)       | The A1 input of the same half (pin 3 or 13)| Connected |

The four HC4514 found here are the column decoders. The other three are the
row decoders. Mark them on a photo.

### 5. Row address

This step settles the pin 6 / pin 7 question.

| From header pin | To                                               | Expected  |
|-----------------|--------------------------------------------------|-----------|
| 5               | Pin 2 (A0) of the three row HC4514               | Connected |
| 7               | Pin 3 (A1) of the same three                     | Connected |
| 6               | Pin 21 (A2) of the same three                    | Connected |
| 8               | A0 of the other 74HC139 half                     | Connected |
| 9               | A1 of the other 74HC139 half                     | Connected |

If pin 6 goes to A1 and pin 7 to A2 instead, the adapter board has the two
bits crossed: keep its wiring and use the Pico row table in the README.

Either way, make the row table match the wiring found here; see "The row
table in the Pico driver" in the README.

### 6. Direction, enables and select

| Find                                                         | Expected                               |
|--------------------------------------------------------------|----------------------------------------|
| Where pin 18 goes                                            | Inputs of one 74HC02 gate; its output to pin 22 (A3) of all seven HC4514 |
| Where pins 10 and 11 go                                      | The two inputs of another 74HC02 gate; its output to /E of the row half of the 74HC139 |
| Which of pins 19 to 24 reaches the column half of the 74HC139| One pin, through a capacitor or a gate, to /E (pin 1 or 15) |
| Pin 1 (LE) of the HC4514                                     | Connected to 5 V                       |
| Pins 12 and the unused ones of 19 to 24                      | Not connected to any chip              |

The select pin found here is the one this panel listens to. Check it on
every panel; they may differ. Note whether there is a capacitor in the path.

Also follow the outputs of one column decoder: Q1 to Q7 (pins 9, 10, 8, 7,
6, 5, 4) should reach the inputs of one driver type and Q9 to Q15 (pins 17,
20, 19, 14, 13, 16, 15) the other. That gives the meaning of each level on
pin 18.

## With 5 V only

Leave the coil supply disconnected for this part. No dot can move.

### 7. Logic supply

First measure the resistance from pin 3 to pin 1. A few ohms means a short,
so stop there. A few hundred ohms or more is normal: the meter sees the
chips and the capacitors on the supply, not one resistor, and the reading
may drift or change with the leads swapped.

Feed 5 V into pin 3 with the current limited to 100 mA, ground on pin 1.
Hold all signal pins (5 to 11 and 13 to 19) at ground through the adapter
board or with jumpers.

Expected: a few milliamps, nothing gets warm, 5 V between pins 24 and 12 of
every HC4514, pins 16 and 8 of the 74HC139 and pins 14 and 7 of the 74HC02.

With all signals low the panel is idle. Step 6 says it should then decode
nothing; check that while the supply is on:

| Measure                                   | Expected                              |
|-------------------------------------------|---------------------------------------|
| 74HC02 pin 1 (row enable)                 | 5 V, so /1E is high and the row half is off |
| 74HC02 pins 11 and 12 (after D1)          | 0 V, held down by R1                  |
| 74HC02 pin 13 (select)                    | 5 V, so /2E is high and the column half is off |
| 74HC139 pins 4 to 7 and 9 to 12           | All 5 V                               |
| Pin 23 (/E) of all seven HC4514           | 5 V, if the 74HC139 drives them; note which Y output reaches which HC4514 |
| Pins 9, 10 and 17 of each HC4514          | 0 V                                   |

### 8. Decoding

Apply an address by hand and measure the outputs of the decoders with the
multimeter. C5 passes only a short pulse from pin 19, so holding pin 19 high
does not keep the columns on. Instead, connect 74HC02 pins 11 and 12 to 5 V
with a jumper: that holds /2E low for as long as the jumper is on. The
jumper draws about 6 mA through R1 (879 Ω) to ground. Keep pin 19 at ground.

| Applied                                             | Expected                                        |
|-----------------------------------------------------|-------------------------------------------------|
| Column address 1, pin 18 low, jumper on             | One output of the first column HC4514 high, all others low |
| Same with pin 18 high                               | The output 8 places away is high instead        |
| Column address 9                                    | The second column HC4514 takes over             |
| Column address 0 or 8                               | An unconnected output (Q0 or Q8), no driver input high |
| Row address 1, pin 10 high                          | One output of the first row HC4514 high         |
| Row address 1, pins 10 and 11 both low              | No row output high                              |
| Jumper off                                          | No column output high                           |

The last two lines matter most: they show that the idle state drives
nothing.

## With coil supply

### 9. Idle current

Keep all signals low. Connect the coil supply to pin 26 at 12 V with the
current limit at about 0.5 A. Both values are a careful starting point, not
a specification.

Expected: almost no current and no dot moves. If current flows, switch off
and go back to step 8.

### 10. First dot

Have the program ready so that the select line can only ever be pulsed, and
goes low again if the program stops.

1. Row address 1, column address 1.
2. ROW-LOW = 1, ROW-HIGH = 0, COL-H/L = 0.
3. One pulse of 200 µs on the select line found in step 6.
4. Reverse the direction (ROW-HIGH = 1, ROW-LOW = 0, COL-H/L = 1) and pulse
   again.

Expected: one dot in a corner flips and flips back. Write down which corner
and which direction shows yellow.

If nothing moves: lengthen the pulse in steps up to 1 ms, then raise the
supply in steps towards 24 V. If several dots move, or a different one,
recheck steps 4 to 6.

### 11. Map the addresses

Flip one dot at a time and note its position.

| Test                              | Expected                                           |
|-----------------------------------|----------------------------------------------------|
| Row 1, column addresses 1 to 31   | 28 dots along one row; 8, 16 and 24 do nothing     |
| Column 1, row addresses 1 to 18   | 16 dots along one column; 8 and 16 do nothing      |
| Row addresses 19 to 31            | Nothing                                            |

Then fill and clear the whole panel once and look for dots that do not flip.

## Results

| Item                                         | Found |
|----------------------------------------------|-------|
| Position of pin 1                            | Confirmed by step 2: ground on 1 and 2, 5 V on 3, coil supply on 26 |
| Pin 4                                        | Not connected to 5 V, nor to pin 4 of the other header |
| Pin 25                                       | Not connected to the coil supply |
| Headers wired pin for pin; select shifted?   | Pins 1–3, 5–11, 13–18 and 26 pin for pin; pins 4, 12 and 25 not connected; pins 20–24 of one header go to 19–23 of the other, shifted by one; its pin 19 and pin 24 of the other go to no pin across |
| Column address (step 4)                      | Pins 13, 14 and 15 to pins 2 (A0), 3 (A1) and 21 (A2) of the same four HC4514 (IC10, IC20, IC30, IC40; IC50, IC60, IC70 are the row decoders); pins 16 and 17 to pins 14 (2A0) and 13 (2A1) of the 74HC139, so its second half is the column half |
| Row address (step 5)                         | Pins 5, 7 and 6 to pins 2 (A0), 3 (A1) and 21 (A2) of the three other HC4514; pins 8 and 9 to pins 2 (1A0) and 3 (1A1) of the 74HC139, so its first half is the row half |
| Pin for ROW1 / pin for ROW2                  | Pin 7 / pin 6 |
| Direction, enables (step 6)                  | 74HC02 orientation checked (GND on pin 7, 5 V on pin 14). Direction: header pin 18 → 74HC02 pins 5 and 6 → pin 4 → pin 22 (A3) of all seven HC4514. Row enable: header pins 10 and 11 → 74HC02 pins 2 and 3 → pin 1 → pin 1 (/1E) of the 74HC139; pin 1 not to A3. Select: header pin 19 → C5 → R3 (read 729 Ω in circuit) → D1 single pin, with R1 from there to ground → 74HC02 pins 11 and 12 → pin 13 → pin 15 (/2E) of the 74HC139. LE of all seven HC4514 on 5 V. Header pins 12 and 20–24 to no chip. Earlier wrong readings: pin 18 on 74HC02 pins 2 and 3; 1404 Ω from 74HC02 pin 13 to 74HC139 pin 15 (probe contact). C5's other pad goes to R3, not directly to R2 or R1; R3's other pad goes to the single pin of D1 and to one pad of R1; R1's other pad (the lower one in the photo) goes to ground, so R1 is the resistor to ground. Decoder outputs to drivers left for step 8 |
| Select pin of each panel                     | Pin 19 of the input header; one panel checked |
| Capacitor in the select path                 | Yes: pin 19 → C5 → R3 → D1 → 74HC02 pins 11 and 12 (inverter) → pin 13 → /2E of the 74HC139; R1 from between R3 and D1 to ground. R1 reads 879 Ω in circuit, R3 729 Ω |
| Pin 18 level for "column sources"            |       |
| Pin 3 to pin 1, no power                     | 624 Ω |
| Idle levels (step 7)                         | 5 V on VCC of the 74HC02, the 74HC139 and all seven HC4514. 74HC02 pin 1 (row enable), pin 4 (direction) and pin 13 (select) at 5 V, pins 11 and 12 at 0 V. 74HC139 pins 4–7 and 9–12 all 5 V. Pin 23 (/E) of all seven HC4514 at 5 V, pins 9, 10 and 17 at 0 V. Pin 4 drives A3, so with pin 18 low A3 is 1 and column address 1 selects Q9. A first reading of 0 V on 74HC02 pins 1 and 14 was bad cabling |
| 74HC139 output to each HC4514 /E             | Header pin 11 high: 74HC02 pin 1 and 74HC139 pin 4 (1Y0) at 0 V, pins 5–7 at 5 V, IC50 /E at 0 V, IC60 and IC70 at 5 V. Header pins 8 and 11 high: IC70 /E at 0 V, so 1Y1 enables IC70 (likely; probe tips too thick for a sure reading). 1Y2, 1Y3 and the column half not checked |
| Header pins 10 and 11 to the 74HC02          | Pin 11 to pin 3, so pin 10 to pin 2 |
| Header levels, 5 V only (step 8)             | Each header pin taken to 5 V on its own, all others on ground: pin 10 or 11 → 74HC02 pin 1 at 0 V; pin 18 → 74HC02 pin 4 at 0 V; pin 8 → 74HC139 pin 2, pin 9 → pin 3, pin 16 → pin 14, pin 17 → pin 13, each 0 V before and 5 V after. Pin 19 held at 5 V leaves 74HC02 pin 13 at 5 V: C5 passes no steady level, so the columns cannot stay on if the select line sticks high |
| Idle current at 5 V / at coil supply         | 4 mA at 5 V, nothing warm, possibly measured with the bad cabling; measure again / |
| Shortest reliable pulse and supply voltage   |       |
| Direction that shows yellow                  | ROW-LOW (pin 11) high, ROW-HIGH (pin 10) low, pin 18 low; black with pin 10 and pin 18 high. Matches "Dot 1/1 on" in the README |
| Corner of row 1, column 1                    | Top left, seen from the front |
