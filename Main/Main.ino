#include <IRsend.h>
#include <WiFi.h>
#include "time.h"
#include "secret.h"

#define POWER 1
#define SILENT 2
#define HIGH_TEMP 3
#define LOW_TEMP 4

const char* ssid = NetworkName;
const char* password = NetworkPassword;
const char* ntpServer = "pool.ntp.org";
const long  gmtOffset_sec = 28800;
const int   daylightOffset_sec = 0;


const uint16_t IR_LED_PIN = 14; // your GPIO

IRsend irsend(IR_LED_PIN);
NetworkServer server(80);
NetworkClient client;


void socketSuccessHandler(int instance){
  client.println("HTTP/1.1 200 OK");
  client.println("Content-Type: application/json");
  client.println("Connection: close");
  client.println();                       // the blank line is required

  switch (instance){
    case 1: //Power
      client.println("{\"result\":\"Success\",\"command\":\"Power\"}");
      break;
    case 2: //Silent
      client.println("{\"result\":\"Success\",\"command\":\"Silent\"}");
      break;
    case 3: //Temp Up
      client.println("{\"result\":\"Success\",\"command\":\"High_Temp\"}");
      break;
    case 4:
      client.println("{\"result\":\"Success\",\"command\":\"Low_Temp\"}");
      break;
  }
}

void sendHttpResponse(int statusCode = 404, int instanceCode = 1) {
    if (statusCode == 200)
        socketSuccessHandler(instanceCode);
    else if (statusCode == 400)
        client.println("HTTP/1.1 400 Bad Request");
    else if (statusCode == 404)
        client.println("HTTP/1.1 404 Not Found");
    else if (statusCode == 500)
        client.println("HTTP/1.1 500 Internal Server Error");
}

void powerButton (){
  Serial.println("Sending Power Signal...");
  irsend.sendNEC(POWER_CODE, MY_BIT);
}

void lowTempButton(){
  Serial.println("Sending Power Signal...");
  irsend.sendNEC(LOW_TEMP_CODE, MY_BIT);
}

void highTempButton(){
  Serial.println("Sending Power Signal...");
  irsend.sendNEC(HIGH_TEMP_CODE, MY_BIT);
}

void silentButton(){
  Serial.println("Sending Power Signal...");
  irsend.sendNEC(SILENT_CODE, MY_BIT);
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
  configTime(gmtOffset_sec, daylightOffset_sec, ntpServer); // Init and get the time

}

void loop() {
  client = server.accept();  // listen for incoming clients

  if (client) {                     // if you get a client,
    Serial.println("Client Connected.");  // print a message out the serial port
    String currentLine = "";        // make a String to hold incoming data from the client
    while (client.connected()) {    // loop while the client's connected
      if (client.available()) {     // if there's bytes to read from the client,
        char c = client.read();     // read a byte, then
        Serial.print(c);            // print it out the serial monitor
        
        if (c != '\r') {  // if you got anything else but a carriage return character,
          currentLine += c;      // add it to the end of the currentLine
        }
        if (c == '\n') {
          if (currentLine.startsWith("Get ")){
            // Check to see if the client request is given 
            if (currentLine.indexOf("GET /Power") >= 0) {
              powerButton();
              socketSuccessHandler(POWER);
            }
            else if (currentLine.indexOf("GET /Silent") >= 0) {
              silentButton();
              socketSuccessHandler(SILENT);
            }
            else if (currentLine.indexOf("GET /Low_Temp") >= 0) {
              lowTempButton();
              socketSuccessHandler(LOW_TEMP);
            }
            else if (currentLine.indexOf("GET /High_Temp") >= 0) {
              highTempButton();
              socketSuccessHandler(HIGH_TEMP);
            }
            else {
              sendHttpResponse();
            }

            break;
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