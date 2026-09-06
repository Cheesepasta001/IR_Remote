#include <IRsend.h>
#include <WiFi.h>
#include "secret.h"

const char* ssid = NetworkName;
const char* password = NetworkPassword;

const bool enableIR = true;

const uint16_t IR_LED_PIN = 4; // your GPIO
IRsend irsend(IR_LED_PIN);

void setup() {
  Serial.begin(115200);
  // Connect to the wifi
  WiFi.mode(WIFI_STA);
  WiFi.begin(ssid, password);

  Serial.print("Connecting to WIFI");
  while (WiFi.status() != WL_CONNECTED) {
    delay(500);
    Serial.print(".");
  }
  
  Serial.println("\nConnected!");
  Serial.print("IP Address: ");
  Serial.println(WiFi.localIP());

  // IR Initiation
  irsend.begin();
}

void loop() {
  if (enableIR){
    irsend.sendNEC(POWER_CODE, 32);
  }
  
  delay(100);
}