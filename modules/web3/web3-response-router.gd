class_name Web3ResponseRouter
extends HttpRouter

# Handle a POST request
func handle_post(request: HttpRequest, response: HttpResponse) -> void:
	print('handle_post')
	
	Panku.notify("Successfully logined")
	Panku.notify(request.get_body_parsed()["wallet"])
	
