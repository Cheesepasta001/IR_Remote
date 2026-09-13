#include <IRsend.h>
#include <WiFi.h>
#include "secret.h"

const char* ssid = NetworkName;
const char* password = NetworkPassword;

const uint16_t IR_LED_PIN = 14; // your GPIO

IRsend irsend(IR_LED_PIN);
NetworkServer server(80);
NetworkClient client;

bool enableIR = true;
int trial = 0;

int powerButton (){
  try{
    Serial.write("Sending Power Signal...");
    irsend.sendNEC(POWER_CODE, MY_BIT);
    client.println("success");
    return 0;
  }
  catch(const std::exception& e) {
        // A fallback catch for any other standard exceptions
        client.println("fail");
        return 1;
  }
}

int lowTempButton(){
  try{
    Serial.write("Sending Power Signal...");
    irsend.sendNEC(LOW_TEMP, MY_BIT);
    client.println("success");
    return 0;
  }
  catch(const std::exception& e) {
        // A fallback catch for any other standard exceptions
        client.println("fail");
        return 1;
  }
}

int highTempButton(){
  try{
    Serial.write("Sending Power Signal...");
    irsend.sendNEC(HIGH_TEMP, MY_BIT);
    client.println("success");
    return 0;
  }
  catch(const std::exception& e) {
        // A fallback catch for any other standard exceptions
        client.println("fail");
        return 1;
  }
}

int silentButton(){
  try{
    Serial.write("Sending Power Signal...");
    irsend.sendNEC(SILENT_CODE, MY_BIT);
    client.println("success");
    return 0;
  }
  catch(const std::exception& e) {
        // A fallback catch for any other standard exceptions
        client.println("fail");
        return 1;
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

  irsend.begin(); // IR Initiation
  server.begin(); // Server Initiation
}

void loop() {
  client = server.accept();  // listen for incoming clients

  if (client) {                     // if you get a client,
    Serial.println("New Client.");  // print a message out the serial port
    String currentLine = "";        // make a String to hold incoming data from the client
    while (client.connected()) {    // loop while the client's connected
      if (client.available()) {     // if there's bytes to read from the client,
        char c = client.read();     // read a byte, then
        Serial.write(c);            // print it out the serial monitor
        
        if (c != '\r') {  // if you got anything else but a carriage return character,
          currentLine += c;      // add it to the end of the currentLine
        }
        
        // Check to see if the client request is given 
        if (currentLine.endsWith("GET /Power")) {
          trial = 0;
          while (trial < 3 && powerButton()){
            trial++;
            delay(500);
          }
        }
        if (currentLine.endsWith("GET /Silent")) {
          trial = 0;
          while (trial < 3 && silentButton()){
            trial++;
            delay(500);
          }
        }
        if (currentLine.endsWith("GET /Low_Temp")) {
          trial = 0;
          while (trial < 3 && lowTempButton()){
            trial++;
            delay(500);
          }
        }
        if (currentLine.endsWith("GET /High_Temp")) {
          trial = 0;
          while (trial < 3 && highTempButton()){
            trial++;
            delay(500);
          }
        }
        
      }
    }
    // close the connection:
    client.stop();
    Serial.println("Client Disconnected.");
  }
  
  delay(100);
}