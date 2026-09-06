#include <IRsend.h>
#include <WiFi.h>
#include <SPI.h>
#include <PubSubClient.h>
#include "secret.h"

const char* ssid = NetworkName;
const char* password = NetworkPassword;

const uint16_t IR_LED_PIN = 4; // your GPIO

IRsend irsend(IR_LED_PIN);

byte mac[]    = MyMac;
IPAddress ip(Myip);
IPAddress server(Myserver);

WiFiClient wifiClient;
PubSubClient client(wifiClient);

bool enableIR = true;

void callback(char* topic, byte* payload, unsigned int length) {
  Serial.print("Message arrived [");
  Serial.print(topic);
  Serial.print("] ");
  for (int i=0;i<length;i++) {
    Serial.print((char)payload[i]);
  }
  Serial.println();
}

void reconnect() {
  // Loop until we're reconnected
  while (!client.connected()) {
    Serial.print("Attempting MQTT connection...");
    // Attempt to connect
    if (client.connect("arduinoClient")) {
      Serial.println("connected");
      // Once connected, publish an announcement...
      client.publish("outTopic","hello world");
      // ... and resubscribe
      client.subscribe("inTopic");
    } else {
      Serial.print("failed, rc=");
      Serial.print(client.state());
      Serial.println(" try again in 5 seconds");
      // Wait 5 seconds before retrying
      delay(5000);
    }
  }
}

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

  // Connect to MQTT server
  client.setServer(server, 1883);
  client.setCallback(callback);

  // IR Initiation
  irsend.begin();
}

void loop() {
  //IR Transmitter
  if (enableIR){
    Serial.write("Sending Power Signal...");
    irsend.sendNEC(POWER_CODE, MY_BIT);
    enableIR=false;
  }

  // Connect to MQTT server if disconnected
  if (!client.connected()){
    Serial.write("Server got disconnected!");
    reconnect();
  }

  client.loop();
  
  delay(100);
}