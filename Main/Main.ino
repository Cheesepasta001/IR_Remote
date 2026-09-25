#include "secret.h"
#define BLYNK_TEMPLATE_ID BLYNK_ID
#define BLYNK_AUTH_TOKEN BLYNK_TOKEN
#define BLYNK_TEMPLATE_NAME BLYNK_NAME

#include <IRsend.h>
#include <WiFi.h>
#include <BlynkSimpleEsp32.h>

#define POWER 1
#define SILENT 2
#define HIGH_TEMP 3
#define LOW_TEMP 4

const char* ssid = NetworkName;
const char* password = NetworkPassword;

const uint16_t IR_LED_PIN = 14; // your GPIO

IRsend irsend(IR_LED_PIN);

void powerButton (){
  Serial.println("Sending Power Signal...");
  irsend.sendNEC(POWER_CODE, MY_BIT);
}

void lowTempButton(){
  Serial.println("Sending - Temp Signal...");
  irsend.sendNEC(LOW_TEMP_CODE, MY_BIT);
}

void highTempButton(){
  Serial.println("Sending + Temp Signal...");
  irsend.sendNEC(HIGH_TEMP_CODE, MY_BIT);
}

void silentButton(){
  Serial.println("Sending Silent Signal...");
  irsend.sendNEC(SILENT_CODE, MY_BIT);
}


BLYNK_WRITE(V0){ // Power Button
  powerButton();
}

BLYNK_WRITE(V1){ // Silent Button
  silentButton();
}

BLYNK_WRITE(V2){ // High Temp Button
  highTempButton();
}

BLYNK_WRITE(V3){ // Low Temp Button
  lowTempButton();
}

void setup() {
  Serial.begin(115200);

  irsend.begin(); // IR Initiation
  Blynk.begin(BLYNK_AUTH_TOKEN, ssid, password);
}

void loop() {
  Blynk.run();
}